//! A terminal screen with its colours: the ANSI text Herdr reads from a
//! pane (`--format ansi`), parsed into runs of styled text per line, so the
//! console can be shown as the terminal shows it (bold, dim, 16/256/true
//! colours, inverse, underline). Anything that is not styling is dropped.

use egui::Color32;

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub fg: Option<Color32>,
    pub bg: Option<Color32>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Default)]
struct Style {
    fg: Option<Color32>,
    bg: Option<Color32>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Style {
    fn span(&self, text: String) -> Span {
        let (fg, bg) = if self.inverse { (self.bg.or(Some(Color32::BLACK)), self.fg.or(Some(Color32::from_gray(222)))) } else { (self.fg, self.bg) };
        Span { text, fg, bg, bold: self.bold, dim: self.dim, italic: self.italic, underline: self.underline }
    }
}

/// The 16 base colours, close to a dark terminal's.
fn base(index: u8) -> Color32 {
    const TABLE: [(u8, u8, u8); 16] = [
        (0, 0, 0), (204, 62, 51), (56, 196, 56), (196, 188, 60), (82, 112, 232), (194, 76, 194), (62, 190, 204), (206, 207, 208),
        (129, 131, 131), (252, 80, 66), (74, 231, 58), (236, 234, 80), (112, 136, 255), (249, 100, 248), (70, 240, 240), (235, 236, 236),
    ];
    let (r, g, b) = TABLE[(index as usize) % 16];
    Color32::from_rgb(r, g, b)
}

/// A colour of the 256-colour palette.
fn indexed(index: u8) -> Color32 {
    match index {
        0..=15 => base(index),
        16..=231 => {
            let i = index - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(step(i / 36), step((i / 6) % 6), step(i % 6))
        }
        _ => { let g = 8 + (index - 232) * 10; Color32::from_rgb(g, g, g) }
    }
}

/// Apply one SGR sequence's parameters to the style.
fn apply_sgr(style: &mut Style, params: &str) {
    let codes: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let codes = if codes.is_empty() { vec![0] } else { codes };
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.inverse = true,
            22 => { style.bold = false; style.dim = false; }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            30..=37 => style.fg = Some(base((codes[i] - 30) as u8)),
            39 => style.fg = None,
            40..=47 => style.bg = Some(base((codes[i] - 40) as u8)),
            49 => style.bg = None,
            90..=97 => style.fg = Some(base((codes[i] - 90 + 8) as u8)),
            100..=107 => style.bg = Some(base((codes[i] - 100 + 8) as u8)),
            38 | 48 => {
                let target = codes[i];
                let color = match codes.get(i + 1) {
                    Some(5) => { let c = codes.get(i + 2).copied().unwrap_or(0); i += 2; Some(indexed(c.min(255) as u8)) }
                    Some(2) => {
                        let (r, g, b) = (codes.get(i + 2).copied().unwrap_or(0), codes.get(i + 3).copied().unwrap_or(0), codes.get(i + 4).copied().unwrap_or(0));
                        i += 4;
                        Some(Color32::from_rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8))
                    }
                    _ => None,
                };
                if target == 38 { style.fg = color } else { style.bg = color }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Lines of styled runs. `\r` is dropped, tabs become spaces, and every
/// escape sequence other than styling (cursor moves, OSC titles…) is
/// skipped.
pub fn parse(text: &str) -> Vec<Vec<Span>> {
    let mut lines: Vec<Vec<Span>> = vec![vec![]];
    let mut style = Style::default();
    let mut run = String::new();
    let mut column = 0usize;
    let flush = |lines: &mut Vec<Vec<Span>>, run: &mut String, style: &Style| {
        if !run.is_empty() {
            let text = std::mem::take(run);
            lines.last_mut().unwrap().push(style.span(text));
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\x1b' {
            flush(&mut lines, &mut run, &style);
            match chars.get(i + 1) {
                Some('[') => {
                    // CSI: parameters, then a final byte in 0x40..=0x7e.
                    let mut j = i + 2;
                    while j < chars.len() && !('\x40'..='\x7e').contains(&chars[j]) { j += 1; }
                    if j < chars.len() && chars[j] == 'm' {
                        let params: String = chars[i + 2..j].iter().collect();
                        apply_sgr(&mut style, &params);
                    }
                    i = j + 1;
                }
                Some(']') => {
                    // OSC: up to BEL or ESC \.
                    let mut j = i + 2;
                    while j < chars.len() && chars[j] != '\x07' && !(chars[j] == '\x1b' && chars.get(j + 1) == Some(&'\\')) { j += 1; }
                    i = if j < chars.len() && chars[j] == '\x1b' { j + 2 } else { j + 1 };
                }
                Some(_) => i += 2,
                None => i += 1,
            }
            continue;
        }
        match c {
            '\n' => { flush(&mut lines, &mut run, &style); lines.push(vec![]); column = 0; }
            '\r' => {}
            '\t' => { let n = 8 - column % 8; run.extend(std::iter::repeat(' ').take(n)); column += n; }
            '\u{a0}' => { run.push(' '); column += 1; }
            c if c.is_control() => {}
            c => { run.push(c); column += 1; }
        }
        i += 1;
    }
    flush(&mut lines, &mut run, &style);
    while lines.len() > 1 && lines.last().map(|l| l.iter().all(|s| s.text.trim().is_empty())).unwrap_or(false) { lines.pop(); }
    lines
}

/// The background a line runs to the terminal's edge with (a TUI's boxes,
/// as Pi paints its tool calls and messages), kept by `reflow` as an empty
/// run at the line's end once its trailing spaces go, for the console to
/// paint across its width.
pub fn line_fill(line: &[Span]) -> Option<Color32> {
    line.last().filter(|s| s.text.is_empty()).and_then(|s| s.bg)
}

/// The plain text of parsed lines (for tests and searches).
pub fn plain(lines: &[Vec<Span>]) -> Vec<String> {
    lines.iter().map(|spans| spans.iter().map(|s| s.text.as_str()).collect()).collect()
}

/// The lines a narrow terminal wrapped, joined again (as `herdr::paragraphs`
/// does for plain text, with the colours kept) so the console can wrap them
/// to its own width. A line that filled the terminal's width continues on
/// the next one unless that one starts a block of its own; a URL cut at the
/// last column goes on without a space.
pub fn reflow(screen: Vec<Vec<Span>>) -> Vec<Vec<Span>> {
    let text = |l: &[Span]| l.iter().map(|s| s.text.as_str()).collect::<String>();
    let widest = screen.iter().map(|l| text(l).trim_end().chars().count()).max().unwrap_or(0);
    if widest < 40 { return screen; }
    let mut out: Vec<Vec<Span>> = Vec::with_capacity(screen.len());
    let (mut open, mut hard) = (false, false);
    let mut above: Option<Color32> = None;
    for mut line in screen {
        let t = text(&line);
        let count = t.trim_end().chars().count();
        let fill = line.last().filter(|s| s.text.ends_with(' ')).and_then(|s| s.bg);
        // A box's lines stay as the program laid them out (a diff, JSON, a
        // table inside a tool call); only plain lines are joined.
        if open && fill.is_none() && above.is_none() && !crate::herdr::starts_block(&t) {
            let last = out.last_mut().expect("an open paragraph");
            // A URL goes on without a space: cut at the last column, or at
            // one of its hyphens (the terminal breaks words there too).
            let tail = text(last.as_slice()).rsplit(' ').next().unwrap_or_default().to_string();
            let glue = tail.contains("://") && (hard || tail.ends_with('-') || t.trim_start().starts_with('-'));
            // The spaces at the seam go: this line's leading ones (the line above lost its trailing ones).
            let mut skip = 0;
            for s in line.iter_mut() {
                let trimmed = s.text.trim_start().to_string();
                if trimmed.is_empty() { skip += 1; } else { s.text = trimmed; break; }
            }
            if !glue { last.push(Span { text: " ".into(), fg: None, bg: None, bold: false, dim: false, italic: false, underline: false }); }
            last.extend(line.into_iter().skip(skip));
        } else {
            out.push(line);
        }
        // Trailing spaces go, so a joined line has no gap and lines compare clean.
        if let Some(last) = out.last_mut() {
            loop {
                let Some(s) = last.last_mut() else { break };
                let n = s.text.trim_end().len();
                s.text.truncate(n);
                if s.text.is_empty() { last.pop(); } else { break; }
            }
            if let Some(bg) = fill { last.push(Span { text: String::new(), fg: None, bg: Some(bg), bold: false, dim: false, italic: false, underline: false }); }
        }
        above = fill;
        open = count > 0 && count + 12 >= widest;
        hard = count >= widest;
    }
    out
}
