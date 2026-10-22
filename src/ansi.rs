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
    let (mut open, mut hard, mut single) = (false, false, false);
    for mut line in screen {
        let t = text(&line);
        let count = t.trim_end().chars().count();
        if open && !crate::herdr::starts_block(&t) {
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
            // Code cut by the wrap stays one piece: no space where a word was cut, and a space in its own colour where it broke at one.
            let code = last.last().is_some_and(is_code) && line.iter().skip(skip).next().is_some_and(is_code);
            // Cut inside a word only when that word filled a line by itself.
            if code && !(hard && single) {
                let style = last.last().cloned().unwrap();
                last.push(Span { text: " ".into(), ..style });
            } else if !glue && !code {
                last.push(Span { text: " ".into(), fg: None, bg: None, bold: false, dim: false, italic: false, underline: false });
            }
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
        }
        open = count > 0 && count + 12 >= widest;
        hard = count >= widest;
        single = !t.trim().contains(' ');
    }
    out
}

/// The colours harnesses paint code in (a command, a path, an id written
/// as `code`): Claude Code's, in its dark and light themes.
const CODE: [Color32; 2] = [Color32::from_rgb(177, 185, 249), Color32::from_rgb(87, 105, 247)];

fn is_code(s: &Span) -> bool { s.fg.is_some_and(|c| CODE.contains(&c)) && !s.text.trim().is_empty() }

/// A piece of code on a console line, to copy with a click: where it is
/// (chars of the line's text) and the whole of it, with the pieces the
/// terminal's wrap left on the lines around it.
#[derive(Clone, Debug, PartialEq)]
pub struct Code { pub start: usize, pub end: usize, pub text: String }

/// The code on each line: runs in a code colour, or text between
/// backticks (for harnesses that print them). A run that reaches the end
/// of its line and goes on at the start of the next is one piece, joined
/// without the break — and without a space where the wrap cut a word.
pub fn codes(lines: &[Vec<Span>]) -> Vec<Vec<Code>> {
    let text = |l: &[Span]| l.iter().map(|s| s.text.as_str()).collect::<String>();
    let widest = lines.iter().map(|l| text(l).trim_end().chars().count()).max().unwrap_or(0);
    let mut out: Vec<Vec<Code>> = lines.iter().map(|line| {
        let mut found: Vec<Code> = vec![];
        // A diff shown by a tool (lines on a coloured ground) is not the agent's prose.
        if line.iter().any(|s| s.bg.is_some()) { return found; }
        let mut at = 0;
        for span in line {
            let n = span.text.chars().count();
            if is_code(span) {
                match found.last_mut() {
                    Some(c) if c.end == at => { c.end += n; c.text.push_str(&span.text); }
                    _ => found.push(Code { start: at, end: at + n, text: span.text.clone() }),
                }
            }
            at += n;
        }
        let chars: Vec<char> = text(line).chars().collect();
        // `like this`, when the harness leaves the backticks in: only a line
        // whose backticks pair up (a file a tool shows, cut by the wrap, does not).
        // Nor a file shown with its line numbers (dim).
        if found.is_empty() && chars.iter().filter(|c| **c == '`').count() % 2 == 0 && !line.iter().any(|s| s.dim) {
            let mut i = 0;
            while let Some(open) = (i..chars.len()).find(|&k| chars[k] == '`') {
                let Some(close) = (open + 1..chars.len()).find(|&k| chars[k] == '`') else { break };
                if close > open + 1 { found.push(Code { start: open, end: close + 1, text: chars[open + 1..close].iter().collect() }); }
                i = close + 1;
            }
        }
        for c in &mut found {
            // The run's own spaces at its edges are layout, not code.
            let lead = c.text.chars().take_while(|ch| ch.is_whitespace()).count();
            c.start += lead;
            c.end -= c.text.chars().rev().take_while(|ch| ch.is_whitespace()).count().min(c.end - c.start);
        }
        found
    }).collect();
    // Pieces the wrap split across lines: a chain of them, top down, then
    // every piece of the chain copies all of it.
    let mut chain: Vec<(usize, usize)> = vec![];
    let mut whole = String::new();
    let finish = |chain: &mut Vec<(usize, usize)>, whole: &mut String, out: &mut Vec<Vec<Code>>| {
        for &(l, k) in chain.iter() { out[l][k].text = whole.clone(); }
        chain.clear();
        whole.clear();
    };
    // Only code painted as code goes on across lines; backticks close on theirs.
    let painted: Vec<bool> = lines.iter().map(|l| l.iter().any(is_code)).collect();
    for i in 0..lines.len() {
        let Some(first) = out[i].first().cloned().filter(|_| painted[i]) else { finish(&mut chain, &mut whole, &mut out); continue };
        let indent = text(&lines[i]).chars().take_while(|c| c.is_whitespace()).count();
        let goes_on = chain.last().is_some_and(|&(l, _)| l + 1 == i) && first.start <= indent;
        if goes_on {
            let above = text(&lines[i - 1]).trim_end().chars().count();
            // A word is cut by the wrap only when it filled the line above by itself.
            let cut = above >= widest && !text(&lines[i - 1]).trim().contains(' ');
            whole = format!("{}{}{}", whole.trim_end(), if cut { "" } else { " " }, first.text.trim_start());
            chain.push((i, 0));
        } else {
            finish(&mut chain, &mut whole, &mut out);
        }
        // The last piece reaches the line's end: it may go on below.
        let last = out[i].len() - 1;
        let reaches = out[i][last].end >= text(&lines[i]).trim_end().chars().count();
        if !(goes_on && last == 0) {
            finish(&mut chain, &mut whole, &mut out);
            if reaches { chain.push((i, last)); whole = out[i][last].text.clone(); }
        } else if !reaches {
            finish(&mut chain, &mut whole, &mut out);
        }
    }
    finish(&mut chain, &mut whole, &mut out);
    for line in &mut out {
        line.retain(|c| c.end > c.start + 1);
        for c in line.iter_mut() { c.text = c.text.trim().to_string(); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_found_whole_even_when_the_terminal_wrapped_it() {
        // As Claude Code paints it: `! gh pr merge 68 --merge` at the end of a line, going on in the next.
        let screen = "  Puedes correr \x1b[38;2;177;185;249m! gh pr merge 68\x1b[0m\n  \x1b[38;2;177;185;249m--merge\x1b[0m desde aquí, o \x1b[38;2;177;185;249mguild_status\x1b[0m.";
        let lines = parse(screen);
        let found = codes(&lines);
        assert_eq!(found[0].len(), 1);
        assert_eq!(found[0][0].text, "! gh pr merge 68 --merge");
        assert_eq!(&plain(&lines)[0].chars().collect::<Vec<_>>()[found[0][0].start..found[0][0].end].iter().collect::<String>(), "! gh pr merge 68");
        assert_eq!(found[1].iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["! gh pr merge 68 --merge", "guild_status"]);
        // A long word the wrap cut at the last column goes on without a space.
        let cut = parse("  \x1b[38;2;177;185;249m/Users/me/agents-pixel-\x1b[0m\n  \x1b[38;2;177;185;249mdungeon/build\x1b[0m and more");
        assert_eq!(codes(&cut)[0][0].text, "/Users/me/agents-pixel-dungeon/build");
        // Backticks left in by a harness; nothing in a line without code.
        assert_eq!(codes(&parse("run `cargo test` now"))[0][0].text, "cargo test");
        assert!(codes(&parse("plain words only"))[0].is_empty());
    }

    #[test]
    fn reflowing_a_wrapped_command_keeps_it_one_piece() {
        // A paragraph as Claude Code wraps it at 44 columns, the command broken at a space.
        let screen = "  Cuando termine la ronda, para integrar\n  todo corre \x1b[38;2;177;185;249mgh pr merge 68\x1b[0m\n  \x1b[38;2;177;185;249m--merge\x1b[0m y luego reinicio el servicio.";
        let joined = reflow(parse(screen));
        let found = codes(&joined);
        let all: Vec<&str> = found.iter().flatten().map(|c| c.text.as_str()).collect();
        assert!(!all.is_empty() && all.iter().all(|c| *c == "gh pr merge 68 --merge"), "every piece copies all of it: {all:?} in {:?}", plain(&joined));
    }

    /// A real screen (`HPD_SCREEN=<file of herdr agent read --format ansi>`): prints what can be copied.
    #[test]
    #[ignore]
    fn codes_on_a_real_screen() {
        let Ok(file) = std::env::var("HPD_SCREEN") else { return };
        let lines = reflow(parse(&std::fs::read_to_string(file).unwrap()));
        for c in codes(&lines).iter().flatten() { println!("{}", c.text); }
    }
}
