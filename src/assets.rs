//! The pixel art, embedded in the binary: the rooms and the hero sheet
//! (art/*.aseprite → Resources/Sprites), the sleeper's head derived from
//! each hero, and the small props drawn from pixel strings (book, scroll,
//! sparkle, "!", tick bubble, z, arms) plus two soft gradients for lights.

use crate::herdr::HERO_ORDER;
use egui::{Color32, ColorImage, Context, Rect, TextureHandle, TextureId, TextureOptions, Vec2};
use std::collections::HashMap;

/// A texture, or part of one, with its size in source pixels.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub tex: TextureId,
    pub uv: Rect,
    pub px: Vec2,
}

pub struct Assets {
    rooms: HashMap<&'static str, TextureHandle>,
    heroes: TextureHandle,
    sleepers: HashMap<&'static str, TextureHandle>,
    arms: HashMap<&'static str, TextureHandle>,
    git_tome: TextureHandle,
    git_tome_arms: HashMap<&'static str, TextureHandle>,
    props: HashMap<&'static str, TextureHandle>,
}

pub const ROOM_FILES: [(&str, &[u8]); 24] = [
    ("guild_room", include_bytes!("../Resources/Sprites/rooms/guild_room.png")),
    ("guild_room_medium", include_bytes!("../Resources/Sprites/rooms/guild_room_medium.png")),
    ("guild_room_small", include_bytes!("../Resources/Sprites/rooms/guild_room_small.png")),
    ("room_working", include_bytes!("../Resources/Sprites/rooms/room_working.png")),
    ("room_working_front", include_bytes!("../Resources/Sprites/rooms/room_working_front.png")),
    ("room_blocked", include_bytes!("../Resources/Sprites/rooms/room_blocked.png")),
    // The infirmary: the usage limit (drawn by scripts/room_limited.py).
    ("room_limited", include_bytes!("../Resources/Sprites/rooms/room_limited.png")),
    ("room_idle", include_bytes!("../Resources/Sprites/rooms/room_idle.png")),
    ("room_done", include_bytes!("../Resources/Sprites/rooms/room_done.png")),
    ("room_unknown", include_bytes!("../Resources/Sprites/rooms/room_unknown.png")),
    // Every room 64 and 40 pixels tall, for full screen's column (scripts/room_sizes.py).
    ("room_working_medium", include_bytes!("../Resources/Sprites/rooms/room_working_medium.png")),
    ("room_working_medium_front", include_bytes!("../Resources/Sprites/rooms/room_working_medium_front.png")),
    ("room_blocked_medium", include_bytes!("../Resources/Sprites/rooms/room_blocked_medium.png")),
    ("room_limited_medium", include_bytes!("../Resources/Sprites/rooms/room_limited_medium.png")),
    ("room_idle_medium", include_bytes!("../Resources/Sprites/rooms/room_idle_medium.png")),
    ("room_done_medium", include_bytes!("../Resources/Sprites/rooms/room_done_medium.png")),
    ("room_unknown_medium", include_bytes!("../Resources/Sprites/rooms/room_unknown_medium.png")),
    ("room_working_small", include_bytes!("../Resources/Sprites/rooms/room_working_small.png")),
    ("room_working_small_front", include_bytes!("../Resources/Sprites/rooms/room_working_small_front.png")),
    ("room_blocked_small", include_bytes!("../Resources/Sprites/rooms/room_blocked_small.png")),
    ("room_limited_small", include_bytes!("../Resources/Sprites/rooms/room_limited_small.png")),
    ("room_idle_small", include_bytes!("../Resources/Sprites/rooms/room_idle_small.png")),
    ("room_done_small", include_bytes!("../Resources/Sprites/rooms/room_done_small.png")),
    ("room_unknown_small", include_bytes!("../Resources/Sprites/rooms/room_unknown_small.png")),
];

pub const HEROES_PNG: &[u8] = include_bytes!("../Resources/Sprites/heroes/heroes.png");

/// The git ledger (art/git_tome.aseprite): a strip of 36x26 frames laid out
/// around the hero standing behind the desk, its centre at (18, 8). The
/// tome lies flat on the desktop; the arm sheet holds the hero's writing arm
/// and quill, frame for frame, in a placeholder sleeve and hand recoloured
/// per hero. Frames: closed, open, six strokes of writing, four of a page
/// being turned.
pub const GIT_TOME_PNG: &[u8] = include_bytes!("../Resources/Sprites/props/git_tome.png");
pub const GIT_TOME_ARM_PNG: &[u8] = include_bytes!("../Resources/Sprites/props/git_tome_arm.png");
pub const GIT_TOME_FRAME: Vec2 = Vec2::new(36.0, 26.0);
pub const GIT_TOME_HERO: Vec2 = Vec2::new(18.0, 8.0);
pub const GIT_TOME_FRAMES: usize = 12;
const GIT_TOME_SLEEVE: Color32 = Color32::from_rgb(215, 118, 67);
const GIT_TOME_HAND: Color32 = Color32::from_rgb(232, 183, 150);

pub fn decode_png(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.as_raw()))
}

const INK: Color32 = Color32::from_rgb(24, 20, 37);

fn rgb(r: f32, g: f32, b: f32) -> Color32 {
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// A texture from rows of characters, each mapped to a colour ('.' is clear).
pub fn pixel_image(rows: &[&str], colors: &[(char, Color32)]) -> ColorImage {
    let w = rows[0].chars().count();
    let h = rows.len();
    let mut image = ColorImage::new([w, h], Color32::TRANSPARENT);
    for (y, line) in rows.iter().enumerate() {
        for (x, ch) in line.chars().enumerate() {
            if let Some((_, color)) = colors.iter().find(|(c, _)| *c == ch) {
                image[(x, y)] = *color;
            }
        }
    }
    image
}

/// A radial gradient: `stops` are (distance 0..1 from the centre, alpha).
fn radial(n: usize, color: Color32, stops: &[(f32, f32)], beyond: f32) -> ColorImage {
    let mut image = ColorImage::new([n, n], Color32::TRANSPARENT);
    let c = n as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt() / c;
            let alpha = if d >= 1.0 {
                beyond
            } else {
                let mut a = stops.last().map(|s| s.1).unwrap_or(0.0);
                for pair in stops.windows(2) {
                    if d >= pair[0].0 && d <= pair[1].0 {
                        let k = (d - pair[0].0) / (pair[1].0 - pair[0].0).max(1e-6);
                        a = pair[0].1 + (pair[1].1 - pair[0].1) * k;
                        break;
                    }
                }
                a
            };
            image[(x, y)] = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha * 255.0) as u8);
        }
    }
    image
}

impl Assets {
    pub fn new(ctx: &Context) -> Assets {
        let nearest = TextureOptions::NEAREST;
        let mut rooms = HashMap::new();
        for (name, bytes) in ROOM_FILES {
            if let Some(image) = decode_png(bytes) {
                rooms.insert(name, ctx.load_texture(name, image, nearest));
            }
        }
        let heroes_image = decode_png(HEROES_PNG).unwrap_or_else(|| ColorImage::new([48, HERO_ORDER.len() * 32], Color32::TRANSPARENT));
        let heroes = ctx.load_texture("heroes", heroes_image.clone(), nearest);
        let heroes_image = &heroes_image;
        let mut sleepers = HashMap::new();
        let mut arms = HashMap::new();
        let strip = |bytes| decode_png(bytes).unwrap_or_else(|| ColorImage::new([GIT_TOME_FRAME.x as usize * GIT_TOME_FRAMES, GIT_TOME_FRAME.y as usize], Color32::TRANSPARENT));
        let git_tome = ctx.load_texture("git_tome", strip(GIT_TOME_PNG), nearest);
        let tome_arm = strip(GIT_TOME_ARM_PNG);
        let mut git_tome_arms = HashMap::new();
        for (index, harness) in HERO_ORDER.iter().enumerate() {
            sleepers.insert(*harness, ctx.load_texture(format!("sleeper/{harness}"), sleeper_image(heroes_image, index), nearest));
            arms.insert(*harness, ctx.load_texture(format!("arm/{harness}"), arm_image(harness), nearest));
            git_tome_arms.insert(*harness, ctx.load_texture(format!("git_tome_arm/{harness}"), tome_arm_image(&tome_arm, harness), nearest));
        }
        let mut props = HashMap::new();
        let mut prop = |name: &'static str, image: ColorImage, options: TextureOptions| {
            props.insert(name, ctx.load_texture(name, image, options));
        };
        let book_colors = [('k', INK), ('R', rgb(0.635, 0.149, 0.2)), ('Y', rgb(0.996, 0.906, 0.38)), ('W', rgb(0.918, 0.831, 0.667)), ('l', rgb(0.353, 0.412, 0.533))];
        prop("book_closed", pixel_image(&["kkkkk", "kRRRk", "kRYRk", "kRRRk", "kkkkk"], &book_colors), nearest);
        prop("book_open", pixel_image(&["kkkk.kkkk", "kWWWkWWWk", "kWlWkWWWk", "kWWWkWlWk", "kRRRkRRRk", ".kkkkkkk."], &book_colors), nearest);
        prop("book_open_flipped", pixel_image(&["kkkk.kkkk", "kWWWkWWWk", "kWWWkWlWk", "kWlWkWWWk", "kRRRkRRRk", ".kkkkkkk."], &book_colors), nearest);
        let scroll_colors = [('k', INK), ('D', rgb(0.722, 0.435, 0.314)), ('W', rgb(0.918, 0.831, 0.667)), ('l', rgb(0.635, 0.149, 0.2))];
        prop("scroll_closed", pixel_image(&["kkkkk", "kDWDk", "kkkkk"], &scroll_colors), nearest);
        prop("scroll_open", pixel_image(&["kkkkkkkkk", "kDWWWWWDk", "kDWlWlWDk", "kDWWWlWDk", "kkkkkkkkk"], &scroll_colors), nearest);
        prop("scroll_open_flipped", pixel_image(&["kkkkkkkkk", "kDWWWWWDk", "kDWlWWlDk", "kDWWlWWDk", "kkkkkkkkk"], &scroll_colors), nearest);
        prop("sparkle", pixel_image(&["..y..", "..w..", "ywWwy", "..w..", "..y.."],
            &[('y', rgb(1.0, 0.91, 0.5)), ('w', Color32::from_rgba_unmultiplied(255, 255, 255, 230)), ('W', Color32::WHITE)]), nearest);
        prop("tick", pixel_image(&[".kkkkkkkkk.", "kwwwwwwwwwk", "kwwwwwwwGwk", "kwwwwwwGgwk", "kwGwwwGgwwk", "kwgGwGgwwwk", "kwwgGgwwwwk", "kwwwgwwwwwk", "kwwwwwwwwwk", ".kkkkkkkkk.", "...kwk.....", "....k......"],
            &[('k', INK), ('w', Color32::WHITE), ('G', rgb(0.388, 0.78, 0.302)), ('g', rgb(0.243, 0.537, 0.282))]), nearest);
        prop("alert", pixel_image(&["..wwwww..", ".wkkkkkw.", "wkRWRRdkw", "wkRWRRdkw", "wkRRRRdkw", ".wkRRdkw.", ".wkRRdkw.", ".wkRRdkw.", ".wkRRdkw.", "..wkRkw..", "..wkRkw..", "..wkkkw..", ".wwkkkww.", ".wkRRdkw.", ".wkRRdkw.", ".wkkkkkw.", "..wwwww.."],
            &[('k', INK), ('R', rgb(0.894, 0.231, 0.267)), ('d', rgb(0.635, 0.149, 0.2)), ('W', Color32::WHITE), ('w', Color32::WHITE)]), nearest);
        prop("z", pixel_image(&["kkkkkk.", "kwwwwk.", "kkkwkk.", ".kwkk..", "kwkkkk.", "kwwwwk.", "kkkkkk."], &[('k', INK), ('w', rgb(0.92, 0.95, 1.0))]), nearest);
        prop("hourglass", pixel_image(&["fffffff", ".fYYYf.", "..fYf..", "...f...", "..fYf..", ".fYYYf.", "fffffff"],
            &[('f', rgb(0.85, 0.62, 0.33)), ('Y', rgb(0.996, 0.906, 0.38))]), nearest);
        // The waiting room's armchair, its colours.
        let chair_colors = [('k', INK), ('w', rgb(0.635, 0.376, 0.235)), ('d', rgb(0.408, 0.22, 0.141)), ('r', rgb(0.769, 0.204, 0.235)), ('p', rgb(0.886, 0.33, 0.337)), ('s', rgb(0.545, 0.118, 0.157))];
        // Seen from its side, for a reader in profile: its back behind the
        // reader's back, its seat, arm and legs over the reader's legs.
        prop("chair_side_back", pixel_image(&["..kkkk", ".krrrk", ".krprk", ".krprk", ".krrrk", ".krrrk", ".krrrk", ".krrrk", ".krrrk", ".ksrrk", "kkkkkk", "kwwddk"], &chair_colors), nearest);
        prop("chair_side_front", pixel_image(&["kkkkkkkkkkkk.", "kwwwwwwwwwwwk", "kkkkkkkkkkkkk", "krrrrrrrrrrrk", "krprrrrrrrrrk", "ksssssssssssk", "kkkkkkkkkkkkk", ".kwk.....kwk.", ".kdk.....kdk."], &chair_colors), nearest);
        // A background shell: an iron cauldron of green brew on little legs.
        prop("cauldron", pixel_image(&["..kkkkkkkk..", ".kGGgGGGgGk.", "kkkkkkkkkkkk", "kIiIIIIIIIdk", "kIiIIIIIIIdk", "kIIIIIIIIIdk", ".kIIIIIIIdk.", "..kkkkkkkk..", "..k......k.."],
            &[('k', INK), ('G', rgb(0.55, 0.92, 0.36)), ('g', rgb(0.3, 0.7, 0.25)), ('I', rgb(0.33, 0.33, 0.4)), ('i', rgb(0.55, 0.56, 0.64)), ('d', rgb(0.2, 0.2, 0.26))]), nearest);
        // The guild's room: each member's chemistry set on its table, the
        // stool it sits on, the director's pulpit.
        let bench_colors = [('k', INK), ('w', rgb(0.635, 0.376, 0.235)), ('d', rgb(0.408, 0.22, 0.141)), ('l', rgb(0.769, 0.506, 0.322)),
            ('R', rgb(0.894, 0.231, 0.267)), ('C', rgb(0.173, 0.91, 0.961)), ('G', rgb(0.388, 0.78, 0.302)), ('g', rgb(0.75, 0.85, 0.9)), ('W', rgb(0.918, 0.831, 0.667)), ('b', rgb(0.353, 0.412, 0.533))];
        prop("chem_set", pixel_image(&["..k....k.....k..", ".kgk..kgk...kgk.", ".kRk.kgCgk..kGk.", "kRRRkkCCCk.kGGGk", "kRRRkkCCCk.kGGGk", "kkkkkkkkkkkkkkkk",
            "kllllllllllllllk", "kwwwwwwwwwwwwwwk", "kddddddddddddddk", "kkkkkkkkkkkkkkkk", ".kd..........dk.", ".kd..........dk.", ".kd..........dk.", ".kk..........kk."], &bench_colors), nearest);
        prop("stool", pixel_image(&["kkkkkkkk", "kllllllk", "kwwwwwwk", "kkkkkkkk", ".kd..dk.", ".kd..dk.", ".kd..dk.", ".kk..kk."], &bench_colors), nearest);
        prop("pulpit", pixel_image(&["......kkkkkk", "...kkkWWWkWk", ".kkWWbWWWkWk", "kWWWWWWWbkWk", "kkkkkkkkkkkk", "kllllllllllk", ".kkkkkkkkkk.", "...kwwwwk...",
            "...kwddwk...", "...kwddwk...", "...kwddwk...", "...kwddwk...", "...kwddwk...", "..kkwwwwkk..", ".kllllllllk.", ".kkkkkkkkkk."], &bench_colors), nearest);
        prop("glow", radial(64, Color32::WHITE, &[(0.0, 1.0), (0.45, 0.35), (1.0, 0.0)], 0.0), TextureOptions::LINEAR);
        prop("darkness", radial(128, rgb(0.02, 0.01, 0.04), &[(0.0, 0.0), (0.1, 0.12), (0.28, 0.6), (0.5, 0.82), (1.0, 0.86)], 0.86), TextureOptions::LINEAR);
        Assets { rooms, heroes, sleepers, arms, git_tome, git_tome_arms, props }
    }

    fn whole(handle: &TextureHandle) -> Sprite {
        let [w, h] = handle.size();
        Sprite { tex: handle.id(), uv: Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), px: Vec2::new(w as f32, h as f32) }
    }

    pub fn room(&self, name: &str) -> Option<Sprite> {
        self.rooms.get(name).map(Assets::whole)
    }

    /// Part of a room, in art pixels (origin top-left).
    pub fn room_part(&self, name: &str, x: f32, y: f32, w: f32, h: f32) -> Option<Sprite> {
        let handle = self.rooms.get(name)?;
        let [tw, th] = handle.size();
        let (tw, th) = (tw as f32, th as f32);
        Some(Sprite { tex: handle.id(), uv: Rect::from_min_max(egui::pos2(x / tw, y / th), egui::pos2((x + w) / tw, (y + h) / th)), px: Vec2::new(w, h) })
    }

    /// heroes.png stacks two 16 px rows per harness: idle (2 frames) and
    /// work (3 frames).
    pub fn hero_frame_count(kind: &str) -> usize {
        if kind == "work" { 3 } else { 2 }
    }

    pub fn hero_frame(&self, harness: &str, kind: &str, index: usize) -> Sprite {
        let rows = (HERO_ORDER.len() * 2) as f32;
        let row = (HERO_ORDER.iter().position(|h| *h == harness).unwrap_or(HERO_ORDER.len() - 1) * 2 + usize::from(kind == "work")) as f32;
        let i = (index % Assets::hero_frame_count(kind)) as f32;
        Sprite {
            tex: self.heroes.id(),
            uv: Rect::from_min_max(egui::pos2(i / 3.0, row / rows), egui::pos2((i + 1.0) / 3.0, (row + 1.0) / rows)),
            px: Vec2::new(16.0, 16.0),
        }
    }

    pub fn sleeper(&self, harness: &str) -> Sprite {
        self.sleepers.get(harness).or_else(|| self.sleepers.get("hero")).map(Assets::whole).unwrap_or_else(|| self.hero_frame(harness, "idle", 0))
    }

    pub fn arm(&self, harness: &str) -> Sprite {
        self.arms.get(harness).or_else(|| self.arms.get("hero")).map(Assets::whole).unwrap()
    }

    /// One frame of the git ledger strip (see GIT_TOME_PNG).
    pub fn git_tome(&self, index: usize) -> Sprite {
        Assets::strip_frame(&self.git_tome, index)
    }

    /// The hero's writing arm over the ledger, the same frame.
    pub fn git_tome_arm(&self, harness: &str, index: usize) -> Sprite {
        Assets::strip_frame(self.git_tome_arms.get(harness).or_else(|| self.git_tome_arms.get("hero")).unwrap(), index)
    }

    fn strip_frame(handle: &TextureHandle, index: usize) -> Sprite {
        let n = GIT_TOME_FRAMES as f32;
        let i = (index % GIT_TOME_FRAMES) as f32;
        Sprite { tex: handle.id(), uv: Rect::from_min_max(egui::pos2(i / n, 0.0), egui::pos2((i + 1.0) / n, 1.0)), px: GIT_TOME_FRAME }
    }

    pub fn prop(&self, name: &str) -> Sprite {
        Assets::whole(&self.props[name])
    }
}

/// The sleeper's head only (the body is under the blanket): the top of the
/// hero's first idle frame, without staff or shield, eyes shut. Each lone
/// dark pixel set in the face (same light colour left and right) becomes a
/// darker shade of that face colour, which reads as a closed lid.
pub const HEAD_CROP: (usize, usize, usize, usize) = (1, 0, 11, 10); // x, y, w, h in sprite pixels

fn sleeper_image(sheet: &ColorImage, harness_index: usize) -> ColorImage {
    let (cx, cy, w, h) = HEAD_CROP;
    let top = harness_index * 32 + cy;
    let mut image = ColorImage::new([w, h], Color32::TRANSPARENT);
    for y in 0..h {
        for x in 0..w {
            let sx = cx + x;
            let sy = top + y;
            if sx < sheet.size[0] && sy < sheet.size[1] {
                image[(x, y)] = sheet[(sx, sy)];
            }
        }
    }
    let dark = |p: Color32| p.a() > 0 && (p.r() as u32 + p.g() as u32 + p.b() as u32) < 160;
    let mut lids = vec![];
    for y in 0..h {
        for x in 1..w - 1 {
            let (c, l, r) = (image[(x, y)], image[(x - 1, y)], image[(x + 1, y)]);
            if dark(c) && !dark(l) && l.a() > 0 && l == r && (l.r() as u32 + l.g() as u32 + l.b() as u32) > 380 {
                lids.push((x, y, l));
            }
        }
    }
    for (x, y, face) in lids {
        image[(x, y)] = Color32::from_rgb((face.r() as f32 * 0.72) as u8, (face.g() as f32 * 0.62) as u8, (face.b() as f32 * 0.62) as u8);
    }
    image
}

/// A hero's sleeve and hand colours.
fn arm_colors(harness: &str) -> (Color32, Color32) {
    let sleeve = match harness {
        "claude" => rgb(0.843, 0.463, 0.263),
        "codex" => rgb(0.227, 0.267, 0.4),
        "kiro" => rgb(0.408, 0.22, 0.424),
        "gemini" => rgb(0.071, 0.306, 0.537),
        "cursor" => rgb(0.32, 0.35, 0.40),
        _ => rgb(0.243, 0.537, 0.282),
    };
    let hand = if harness == "codex" { rgb(0.753, 0.796, 0.863) } else { rgb(0.91, 0.718, 0.588) };
    (sleeve, hand)
}

/// The ledger's writing arm in this hero's sleeve and hand.
fn tome_arm_image(sheet: &ColorImage, harness: &str) -> ColorImage {
    let (sleeve, hand) = arm_colors(harness);
    let mut image = sheet.clone();
    for p in image.pixels.iter_mut() {
        if *p == GIT_TOME_SLEEVE { *p = sleeve } else if *p == GIT_TOME_HAND { *p = hand }
    }
    image
}

/// A 3x7 raised arm in the hero's sleeve colour, hand at the top.
fn arm_image(harness: &str) -> ColorImage {
    let (sleeve, hand) = arm_colors(harness);
    pixel_image(&["kHk", "kHk", "kSk", "kSk", "kSk", "kSk", ".k."], &[('k', INK), ('H', hand), ('S', sleeve)])
}
