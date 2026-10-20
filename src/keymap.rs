//! Keyboard shortcuts that can be reassigned in the settings: the actions,
//! their default keys, and the text form kept in settings.toml
//! ("Ctrl+Shift+S", "Left", "Num0"). Esc, F1–F9 (quick folders) and
//! Shift+0…4 (channels) depend on context and stay fixed.

use crate::i18n::tr;
use std::collections::{BTreeMap, HashMap};

/// A key with its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Key {
    pub const fn plain(vk: u16) -> Self {
        Self {
            vk,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    pub const fn ctrl(vk: u16) -> Self {
        Self {
            vk,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    pub const fn shift(vk: u16) -> Self {
        Self {
            vk,
            ctrl: false,
            shift: true,
            alt: false,
        }
    }

    pub const fn ctrl_shift(vk: u16) -> Self {
        Self {
            vk,
            ctrl: true,
            shift: true,
            alt: false,
        }
    }

    /// The text kept in settings.toml.
    pub fn to_text(self) -> String {
        let mut s = String::new();
        for (on, m) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
        ] {
            if on {
                s.push_str(m);
            }
        }
        s.push_str(&token(self.vk));
        s
    }

    pub fn from_text(text: &str) -> Option<Self> {
        let mut k = Key::plain(0);
        let mut rest = text.trim();
        loop {
            if let Some(r) = rest.strip_prefix("Ctrl+") {
                k.ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Shift+") {
                k.shift = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Alt+") {
                k.alt = true;
                rest = r;
            } else {
                break;
            }
        }
        k.vk = vk_of(rest)?;
        Some(k)
    }

    /// What the settings card shows: "Ctrl+Shift+S", "←", "Пробел".
    pub fn label(self) -> String {
        let mut s = String::new();
        for (on, m) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
        ] {
            if on {
                s.push_str(m);
            }
        }
        let name = match self.vk {
            0x25 => "←".to_string(),
            0x27 => "→".to_string(),
            0x26 => "↑".to_string(),
            0x28 => "↓".to_string(),
            0x20 => tr("Пробел").to_string(),
            0x0D => "Enter".to_string(),
            0x2E => "Del".to_string(),
            0x60..=0x69 => format!("Num {}", self.vk - 0x60),
            0x6B => "Num +".to_string(),
            0x6D => "Num −".to_string(),
            0x6A => "Num *".to_string(),
            0x6F => "Num /".to_string(),
            0xBB => "+".to_string(),
            0xBD => "−".to_string(),
            0xBC => ",".to_string(),
            0xBE => ".".to_string(),
            0xDB => "[".to_string(),
            0xDD => "]".to_string(),
            0xBA => ";".to_string(),
            0xBF => "/".to_string(),
            0xC0 => "`".to_string(),
            0xDC => "\\".to_string(),
            0xDE => "'".to_string(),
            vk => token(vk),
        };
        s.push_str(&name);
        s
    }
}

/// Named keys, both ways (storage tokens).
const NAMES: &[(u16, &str)] = &[
    (0x08, "Backspace"),
    (0x09, "Tab"),
    (0x0D, "Enter"),
    (0x20, "Space"),
    (0x21, "PgUp"),
    (0x22, "PgDn"),
    (0x23, "End"),
    (0x24, "Home"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x6A, "NumMul"),
    (0x6B, "NumAdd"),
    (0x6D, "NumSub"),
    (0x6E, "NumDecimal"),
    (0x6F, "NumDiv"),
    (0xBA, "Semicolon"),
    (0xBB, "Plus"),
    (0xBC, "Comma"),
    (0xBD, "Minus"),
    (0xBE, "Period"),
    (0xBF, "Slash"),
    (0xC0, "Backquote"),
    (0xDB, "LBracket"),
    (0xDC, "Backslash"),
    (0xDD, "RBracket"),
    (0xDE, "Quote"),
];

fn token(vk: u16) -> String {
    if let Some((_, n)) = NAMES.iter().find(|(v, _)| *v == vk) {
        return (*n).to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("VK{vk:02X}"),
    }
}

fn vk_of(t: &str) -> Option<u16> {
    if let Some((v, _)) = NAMES.iter().find(|(_, n)| n.eq_ignore_ascii_case(t)) {
        return Some(*v);
    }
    let b = t.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_digit() || b[0].is_ascii_alphabetic()) {
        return Some(u16::from(b[0].to_ascii_uppercase()));
    }
    if let Some(n) = t.strip_prefix("Num").and_then(|n| n.parse::<u16>().ok()) {
        return (n <= 9).then_some(0x60 + n);
    }
    if let Some(n) = t.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then_some(0x6F + n);
    }
    t.strip_prefix("VK")
        .and_then(|h| u16::from_str_radix(h, 16).ok())
}

/// Actions that can be reassigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    Prev,
    Next,
    First,
    Last,
    ZoomIn,
    ZoomOut,
    Fit,
    ActualSize,
    ToggleZoom,
    Fullscreen,
    ToggleMode,
    RotateCw,
    RotateCcw,
    FlipH,
    FlipV,
    TiltLeft,
    TiltRight,
    TiltLeftFine,
    TiltRightFine,
    Filmstrip,
    Checker,
    Smoothing,
    Grid,
    Histogram,
    Info,
    Slideshow,
    Compare,
    CompareLayout,
    PlayPause,
    FramePrev,
    FrameNext,
    Editor,
    Delete,
    Save,
    SaveAs,
    Undo,
    CopyImage,
    CopyPath,
    Rate0,
    Rate1,
    Rate2,
    Rate3,
    Rate4,
    Rate5,
    RatingFilter,
    Crop,
    Adjust,
    Wireframe,
    Section,
    Measure,
    LayFlat,
    Objects,
}

impl Command {
    pub const ALL: [Command; 52] = {
        use Command::*;
        [
            Prev,
            Next,
            First,
            Last,
            ZoomIn,
            ZoomOut,
            Fit,
            ActualSize,
            ToggleZoom,
            Fullscreen,
            ToggleMode,
            RotateCw,
            RotateCcw,
            FlipH,
            FlipV,
            TiltLeft,
            TiltRight,
            TiltLeftFine,
            TiltRightFine,
            Filmstrip,
            Checker,
            Smoothing,
            Grid,
            Histogram,
            Info,
            Slideshow,
            Compare,
            CompareLayout,
            PlayPause,
            FramePrev,
            FrameNext,
            Editor,
            Delete,
            Save,
            SaveAs,
            Undo,
            CopyImage,
            CopyPath,
            Rate1,
            Rate2,
            Rate3,
            Rate4,
            Rate5,
            Rate0,
            RatingFilter,
            Crop,
            Adjust,
            Wireframe,
            Section,
            Measure,
            LayFlat,
            Objects,
        ]
    };

    /// Name in settings.toml.
    pub fn id(self) -> &'static str {
        use Command::*;
        match self {
            Prev => "prev",
            Next => "next",
            First => "first",
            Last => "last",
            ZoomIn => "zoom_in",
            ZoomOut => "zoom_out",
            Fit => "fit",
            ActualSize => "actual_size",
            ToggleZoom => "toggle_zoom",
            Fullscreen => "fullscreen",
            ToggleMode => "toggle_mode",
            RotateCw => "rotate_cw",
            RotateCcw => "rotate_ccw",
            FlipH => "flip_h",
            FlipV => "flip_v",
            TiltLeft => "tilt_left",
            TiltRight => "tilt_right",
            TiltLeftFine => "tilt_left_fine",
            TiltRightFine => "tilt_right_fine",
            Filmstrip => "filmstrip",
            Checker => "checker",
            Smoothing => "smoothing",
            Grid => "grid",
            Histogram => "histogram",
            Info => "info",
            Slideshow => "slideshow",
            Compare => "compare",
            CompareLayout => "compare_layout",
            PlayPause => "play_pause",
            FramePrev => "frame_prev",
            FrameNext => "frame_next",
            Editor => "editor",
            Delete => "delete",
            Save => "save",
            SaveAs => "save_as",
            Undo => "undo",
            CopyImage => "copy_image",
            CopyPath => "copy_path",
            Rate0 => "rate_0",
            Rate1 => "rate_1",
            Rate2 => "rate_2",
            Rate3 => "rate_3",
            Rate4 => "rate_4",
            Rate5 => "rate_5",
            RatingFilter => "rating_filter",
            Crop => "crop",
            Adjust => "adjust",
            Wireframe => "wireframe",
            Section => "section",
            Measure => "measure",
            LayFlat => "lay_flat",
            Objects => "objects",
        }
    }

    /// Name in the settings card.
    pub fn label(self) -> &'static str {
        use Command::*;
        match self {
            Prev => tr("Предыдущее фото"),
            Next => tr("Следующее фото"),
            First => tr("Первое в папке"),
            Last => tr("Последнее в папке"),
            ZoomIn => tr("Увеличить"),
            ZoomOut => tr("Уменьшить"),
            Fit => tr("Вписать"),
            ActualSize => "100 %",
            ToggleZoom => tr("Вписать ↔ 100 %"),
            Fullscreen => tr("Во весь экран"),
            ToggleMode => tr("Экран ↔ окно"),
            RotateCw => tr("Повернуть вправо"),
            RotateCcw => tr("Повернуть влево"),
            FlipH => tr("Отразить по горизонтали"),
            FlipV => tr("Отразить по вертикали"),
            TiltLeft => tr("Наклон −1°"),
            TiltRight => tr("Наклон +1°"),
            TiltLeftFine => tr("Наклон −0,1°"),
            TiltRightFine => tr("Наклон +0,1°"),
            Filmstrip => tr("Лента миниатюр"),
            Checker => tr("Фон прозрачности"),
            Smoothing => tr("Сглаживание"),
            Grid => tr("Сетка пикселей"),
            Histogram => tr("Гистограмма"),
            Info => tr("Информация"),
            Slideshow => tr("Слайд-шоу"),
            Compare => tr("Сравнение"),
            CompareLayout => tr("Вид сравнения"),
            PlayPause => tr("Пауза анимации"),
            FramePrev => tr("Кадр назад"),
            FrameNext => tr("Кадр вперёд"),
            Editor => tr("Открыть в редакторе"),
            Delete => tr("В корзину"),
            Save => tr("Сохранить"),
            SaveAs => tr("Сохранить как"),
            Undo => tr("Отменить сохранение"),
            CopyImage => tr("Копировать изображение"),
            CopyPath => tr("Копировать путь"),
            Rate0 => tr("Снять рейтинг"),
            Rate1 => tr("Рейтинг ★"),
            Rate2 => tr("Рейтинг ★★"),
            Rate3 => tr("Рейтинг ★★★"),
            Rate4 => tr("Рейтинг ★★★★"),
            Rate5 => tr("Рейтинг ★★★★★"),
            RatingFilter => tr("Фильтр по рейтингу"),
            Crop => tr("Обрезать"),
            Adjust => tr("Коррекция"),
            Wireframe => tr("Каркас модели"),
            Section => tr("Разрез модели"),
            Measure => tr("Линейка"),
            LayFlat => tr("Положить гранью на стол"),
            Objects => tr("Объекты проекта"),
        }
    }

    /// The keys this action had before shortcuts could be reassigned.
    pub fn defaults(self) -> Vec<Key> {
        use Command::*;
        let c = |ch: u8| u16::from(ch);
        match self {
            Prev => vec![Key::plain(0x25), Key::plain(0x21)],
            Next => vec![Key::plain(0x27), Key::plain(0x22)],
            First => vec![Key::plain(0x24)],
            Last => vec![Key::plain(0x23)],
            ZoomIn => vec![Key::plain(0xBB), Key::plain(0x6B)],
            ZoomOut => vec![Key::plain(0xBD), Key::plain(0x6D)],
            Fit => vec![Key::plain(c(b'0')), Key::plain(0x60)],
            ActualSize => vec![Key::plain(c(b'1')), Key::plain(0x61)],
            ToggleZoom => vec![Key::plain(c(b'Z'))],
            Fullscreen => vec![Key::plain(0x0D)],
            ToggleMode => vec![Key::plain(0x7A)],
            RotateCw => vec![Key::plain(c(b'R'))],
            RotateCcw => vec![Key::plain(c(b'L'))],
            FlipH => vec![Key::plain(c(b'M'))],
            FlipV => vec![Key::shift(c(b'M'))],
            TiltLeft => vec![Key::plain(0xDB)],
            TiltRight => vec![Key::plain(0xDD)],
            TiltLeftFine => vec![Key::shift(0xDB)],
            TiltRightFine => vec![Key::shift(0xDD)],
            Filmstrip => vec![Key::plain(c(b'F'))],
            Checker => vec![Key::plain(c(b'T'))],
            Smoothing => vec![Key::plain(c(b'N'))],
            Grid => vec![Key::plain(c(b'G'))],
            Histogram => vec![Key::plain(c(b'H'))],
            Info => vec![Key::plain(c(b'I'))],
            Slideshow => vec![Key::plain(c(b'S')), Key::plain(0x74)],
            Compare => vec![Key::plain(c(b'C'))],
            CompareLayout => vec![Key::plain(c(b'V'))],
            PlayPause => vec![Key::plain(0x20)],
            FramePrev => vec![Key::plain(0xBC)],
            FrameNext => vec![Key::plain(0xBE)],
            Editor => vec![Key::plain(c(b'E'))],
            Delete => vec![Key::plain(0x2E)],
            Save => vec![Key::ctrl(c(b'S'))],
            SaveAs => vec![Key::ctrl_shift(c(b'S'))],
            Undo => vec![Key::ctrl(c(b'Z'))],
            CopyImage => vec![Key::ctrl(c(b'C'))],
            CopyPath => vec![Key::ctrl_shift(c(b'C'))],
            Rate0 => vec![Key::ctrl(c(b'0'))],
            Rate1 => vec![Key::ctrl(c(b'1'))],
            Rate2 => vec![Key::ctrl(c(b'2'))],
            Rate3 => vec![Key::ctrl(c(b'3'))],
            Rate4 => vec![Key::ctrl(c(b'4'))],
            Rate5 => vec![Key::ctrl(c(b'5'))],
            RatingFilter => vec![Key::ctrl(c(b'F'))],
            Crop => vec![Key::plain(c(b'X'))],
            Adjust => vec![Key::plain(c(b'A'))],
            Wireframe => vec![Key::plain(c(b'W'))],
            Section => vec![Key::plain(c(b'P'))],
            Measure => vec![Key::plain(c(b'D'))],
            LayFlat => vec![Key::plain(c(b'B'))],
            Objects => vec![Key::plain(c(b'O'))],
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Keys a single action can have.
pub const SLOTS: usize = 2;

/// Overrides as kept in settings.toml: action id → its keys as text (an
/// empty list: no key). Actions not listed keep their defaults.
pub type Overrides = BTreeMap<String, Vec<String>>;

/// The keys of every action.
#[derive(Clone, Debug)]
pub struct Keymap {
    keys: HashMap<Command, Vec<Key>>,
    lookup: HashMap<Key, Command>,
}

impl Keymap {
    pub fn new(overrides: &Overrides) -> Self {
        let mut keys = HashMap::new();
        for c in Command::ALL {
            let list = match overrides.get(c.id()) {
                Some(v) => v.iter().filter_map(|t| Key::from_text(t)).collect(),
                None => c.defaults(),
            };
            keys.insert(c, list);
        }
        let mut lookup = HashMap::new();
        // In the order of the list, so that a duplicate goes to the first.
        for c in Command::ALL {
            for k in &keys[&c] {
                lookup.entry(*k).or_insert(c);
            }
        }
        Self { keys, lookup }
    }

    pub fn keys(&self, c: Command) -> &[Key] {
        self.keys.get(&c).map_or(&[], Vec::as_slice)
    }

    /// The action of a key press. With Shift held and nothing bound to the
    /// exact combination, the key without Shift counts ("+" is Shift+"="
    /// on many layouts).
    pub fn get(&self, k: Key) -> Option<Command> {
        self.lookup.get(&k).copied().or_else(|| {
            k.shift
                .then(|| self.lookup.get(&Key { shift: false, ..k }).copied())
                .flatten()
        })
    }
}

/// Puts `key` into slot `slot` of `cmd` (taking it from any other action);
/// `None` clears the slot. Updates `overrides` so that only actions that
/// differ from their defaults are listed.
pub fn assign(overrides: &mut Overrides, cmd: Command, slot: usize, key: Option<Key>) {
    let map = Keymap::new(overrides);
    let mut all: HashMap<Command, Vec<Key>> = Command::ALL
        .into_iter()
        .map(|c| (c, map.keys(c).to_vec()))
        .collect();
    if let Some(k) = key {
        for list in all.values_mut() {
            list.retain(|x| *x != k);
        }
    }
    let list = all.entry(cmd).or_default();
    match key {
        Some(k) if slot < list.len() => list[slot] = k,
        Some(k) => list.push(k),
        None if slot < list.len() => {
            list.remove(slot);
        }
        None => {}
    }
    list.truncate(SLOTS);
    overrides.clear();
    for c in Command::ALL {
        let list = &all[&c];
        if *list != c.defaults() {
            overrides.insert(
                c.id().to_string(),
                list.iter().map(|k| k.to_text()).collect(),
            );
        }
    }
}

/// Drops entries for unknown actions and unreadable keys (hand-edited
/// files).
pub fn sanitize(overrides: &mut Overrides) {
    overrides.retain(|id, _| Command::from_id(id).is_some());
    for v in overrides.values_mut() {
        v.retain(|t| Key::from_text(t).is_some());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        for k in [
            Key::ctrl_shift(u16::from(b'S')),
            Key::plain(0x25),
            Key::plain(0x60),
            Key::shift(0xDB),
            Key::plain(0x74),
            Key::plain(0xE2),
        ] {
            assert_eq!(Key::from_text(&k.to_text()), Some(k), "{}", k.to_text());
        }
        assert_eq!(Key::ctrl_shift(u16::from(b'S')).to_text(), "Ctrl+Shift+S");
        assert_eq!(Key::from_text("nonsense"), None);
    }

    #[test]
    fn defaults_are_unique_and_shift_falls_back() {
        let m = Keymap::new(&Overrides::new());
        let mut seen = HashMap::new();
        for c in Command::ALL {
            for k in c.defaults() {
                assert!(seen.insert(k, c).is_none(), "{k:?} twice");
            }
        }
        assert_eq!(m.get(Key::plain(u16::from(b'M'))), Some(Command::FlipH));
        assert_eq!(m.get(Key::shift(u16::from(b'M'))), Some(Command::FlipV));
        // Shift+"=" is "+" on US layouts.
        assert_eq!(m.get(Key::shift(0xBB)), Some(Command::ZoomIn));
        assert_eq!(m.get(Key::ctrl(u16::from(b'R'))), None);
    }

    #[test]
    fn assigning_takes_the_key_from_others() {
        let mut o = Overrides::new();
        let r = Key::plain(u16::from(b'R'));
        assign(&mut o, Command::Next, 1, Some(r));
        let m = Keymap::new(&o);
        assert_eq!(m.get(r), Some(Command::Next));
        assert_eq!(m.keys(Command::Next), [Key::plain(0x27), r]);
        assert!(m.keys(Command::RotateCw).is_empty());
        // Back to the defaults: nothing left to store.
        assign(&mut o, Command::Next, 1, Some(Key::plain(0x22)));
        assign(&mut o, Command::RotateCw, 0, Some(r));
        assert!(o.is_empty(), "{o:?}");
        // Clearing a slot.
        assign(&mut o, Command::Prev, 0, None);
        assert_eq!(Keymap::new(&o).keys(Command::Prev), [Key::plain(0x21)]);
    }
}
