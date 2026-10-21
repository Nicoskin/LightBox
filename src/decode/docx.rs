//! Word documents (DOCX) as paged documents: the text, its formatting,
//! tables and pictures are read from the package and laid out with
//! DirectWrite on pages of the size and margins the file asks for, then a
//! page is drawn into a bitmap like a PDF page. Headers, footers, footnotes,
//! text boxes and numbering formats are not drawn (list items get a
//! bullet). Runs on worker threads.

use std::collections::HashMap;
use std::path::Path;

use roxmltree::{Document, Node};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1RenderTarget,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_LINE_METRICS,
    DWRITE_LINE_SPACING_METHOD_PROPORTIONAL, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_JUSTIFIED, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_WRAP, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad, WICBitmapDitherTypeNone,
    WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::core::{HSTRING, Interface as _};
use windows_numerics::{Matrix3x2, Vector2};

use super::text::{Ctx, err};
use super::{DecodeError, Raw, RawImage, SizeHint};

/// Twips (1/1440 inch) → DIPs (1/96 inch).
fn twips(v: f32) -> f32 {
    v / 15.0
}

/// EMUs (914 400 per inch) → DIPs.
fn emu(v: f32) -> f32 {
    v / 9525.0
}

/// Half-points → DIPs.
fn half_points(v: f32) -> f32 {
    v / 2.0 * 96.0 / 72.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Character and paragraph properties as a style or a run states them
/// (`None`: inherited).
#[derive(Clone, Debug, Default)]
struct Props {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    /// DIPs.
    size: Option<f32>,
    color: Option<[f32; 3]>,
    font: Option<String>,
    align: Option<Align>,
    before: Option<f32>,
    after: Option<f32>,
    /// Line height as a multiple of single.
    line: Option<f32>,
    left: Option<f32>,
    right: Option<f32>,
}

impl Props {
    /// `self` with `over` laid on top.
    fn with(&self, over: &Props) -> Props {
        Props {
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            underline: over.underline.or(self.underline),
            strike: over.strike.or(self.strike),
            size: over.size.or(self.size),
            color: over.color.or(self.color),
            font: over.font.clone().or_else(|| self.font.clone()),
            align: over.align.or(self.align),
            before: over.before.or(self.before),
            after: over.after.or(self.after),
            line: over.line.or(self.line),
            left: over.left.or(self.left),
            right: over.right.or(self.right),
        }
    }
}

/// A formatted run of a paragraph's text (UTF-16 offsets).
#[derive(Clone, Debug)]
struct Run {
    start: u32,
    len: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    size: f32,
    color: [f32; 3],
    font: String,
}

#[derive(Clone, Debug, Default)]
struct Para {
    text: Vec<u16>,
    runs: Vec<Run>,
    align: Align,
    before: f32,
    after: f32,
    line: f32,
    left: f32,
    right: f32,
    /// Table rows: cells at equal tab stops this far apart.
    tab: Option<f32>,
    page_break_before: bool,
}

#[derive(Debug)]
enum Block {
    Para(Para),
    Image {
        data: Vec<u8>,
        size: (f32, f32),
        align: Align,
    },
}

/// A block (or part of a paragraph) on a page.
#[derive(Clone, Copy, Debug)]
struct Piece {
    block: usize,
    /// Top on the page's text area, and the part of the block shown.
    y: f32,
    from: f32,
    height: f32,
}

pub struct DocxDoc {
    /// Page size and margins (left, top, right, bottom), DIPs.
    page: (f32, f32),
    margin: [f32; 4],
    blocks: Vec<Block>,
    pages: Vec<Vec<Piece>>,
}

/// `true` for Word documents.
pub fn is_docx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
}

/// An attribute by its local name (w:val, r:embed…).
fn attr<'a>(n: Node<'a, '_>, local: &str) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

fn child<'a, 'i>(n: Node<'a, 'i>, local: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.tag_name().name() == local)
}

fn number(n: Node, local: &str) -> Option<f32> {
    attr(n, local).and_then(|v| v.trim().parse::<f32>().ok())
}

/// On / off properties: present means on unless w:val says otherwise.
fn toggle(n: Node) -> bool {
    !matches!(attr(n, "val"), Some("0" | "false" | "none" | "off"))
}

fn hex_color(v: &str) -> Option<[f32; 3]> {
    if v.len() != 6 {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?].map(|c| f32::from(c) / 255.0))
}

/// Run properties (w:rPr).
fn run_props(rpr: Node) -> Props {
    let mut p = Props::default();
    for c in rpr.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "b" => p.bold = Some(toggle(c)),
            "i" => p.italic = Some(toggle(c)),
            "u" => p.underline = Some(toggle(c)),
            "strike" | "dstrike" => p.strike = Some(toggle(c)),
            "sz" => p.size = number(c, "val").map(half_points),
            "color" => p.color = attr(c, "val").and_then(hex_color),
            "rFonts" => {
                p.font = ["ascii", "hAnsi", "cs"]
                    .iter()
                    .find_map(|k| attr(c, k))
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    p
}

/// Paragraph properties (w:pPr), without its run properties.
fn para_props(ppr: Node) -> Props {
    let mut p = Props::default();
    for c in ppr.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "jc" => {
                p.align = attr(c, "val").map(|v| match v {
                    "center" => Align::Center,
                    "right" | "end" => Align::Right,
                    "both" | "distribute" => Align::Justify,
                    _ => Align::Left,
                });
            }
            "spacing" => {
                p.before = number(c, "before").map(twips);
                p.after = number(c, "after").map(twips);
                if matches!(attr(c, "lineRule"), None | Some("auto")) {
                    p.line = number(c, "line").map(|l| l / 240.0);
                }
            }
            "ind" => {
                p.left = number(c, "left").or(number(c, "start")).map(twips);
                p.right = number(c, "right").or(number(c, "end")).map(twips);
            }
            _ => {}
        }
    }
    p
}

/// Paragraph styles (with what they are based on resolved) and the
/// document defaults.
struct Styles {
    defaults: Props,
    paragraph: HashMap<String, Props>,
}

impl Styles {
    fn parse(xml: Option<&str>) -> Styles {
        // Word's own defaults when the file says nothing.
        let mut defaults = Props {
            size: Some(half_points(22.0)),
            font: Some("Calibri".into()),
            after: Some(twips(160.0)),
            line: Some(259.0 / 240.0),
            ..Props::default()
        };
        let mut raw: HashMap<String, (Option<String>, Props)> = HashMap::new();
        if let Some(doc) = xml.and_then(|x| Document::parse(x).ok()) {
            let root = doc.root_element();
            if let Some(dd) = child(root, "docDefaults") {
                if let Some(rpr) = child(dd, "rPrDefault").and_then(|n| child(n, "rPr")) {
                    defaults = defaults.with(&run_props(rpr));
                }
                if let Some(ppr) = child(dd, "pPrDefault").and_then(|n| child(n, "pPr")) {
                    defaults = defaults.with(&para_props(ppr));
                }
            }
            for s in root.children().filter(|n| n.has_tag_name("style")) {
                if attr(s, "type") != Some("paragraph") {
                    continue;
                }
                let Some(id) = attr(s, "styleId") else {
                    continue;
                };
                let mut p = Props::default();
                if let Some(ppr) = child(s, "pPr") {
                    p = p.with(&para_props(ppr));
                }
                if let Some(rpr) = child(s, "rPr") {
                    p = p.with(&run_props(rpr));
                }
                let based = child(s, "basedOn")
                    .and_then(|b| attr(b, "val"))
                    .map(str::to_string);
                raw.insert(id.to_string(), (based, p));
            }
        }
        let resolve = |id: &str| {
            let mut chain = Vec::new();
            let mut cur = Some(id.to_string());
            while let Some(c) = cur {
                if chain.len() > 16 {
                    break;
                }
                let Some((based, p)) = raw.get(&c) else {
                    break;
                };
                chain.push(p.clone());
                cur = based.clone();
            }
            chain
                .iter()
                .rev()
                .fold(Props::default(), |acc, p| acc.with(p))
        };
        let paragraph = raw.keys().map(|k| (k.clone(), resolve(k))).collect();
        Styles {
            defaults,
            paragraph,
        }
    }

    /// The properties of a paragraph with this style (and the defaults).
    fn of(&self, style: Option<&str>) -> Props {
        let named = style
            .and_then(|s| self.paragraph.get(s))
            .or_else(|| self.paragraph.get("Normal"));
        match named {
            Some(p) => self.defaults.with(p),
            None => self.defaults.clone(),
        }
    }
}

/// Builds the blocks of the body.
struct Reader<'a> {
    styles: &'a Styles,
    rels: &'a HashMap<String, String>,
    files: &'a HashMap<String, Vec<u8>>,
    blocks: Vec<Block>,
    content_width: f32,
}

impl Reader<'_> {
    fn image(&self, drawing: Node) -> Option<(Vec<u8>, (f32, f32))> {
        let extent = drawing.descendants().find(|n| n.has_tag_name("extent"))?;
        let size = (emu(number(extent, "cx")?), emu(number(extent, "cy")?));
        let blip = drawing.descendants().find(|n| n.has_tag_name("blip"))?;
        let target = self.rels.get(attr(blip, "embed")?)?;
        let name = format!(
            "word/{}",
            target.trim_start_matches('/').trim_start_matches("word/")
        );
        let data = self
            .files
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&name))
            .map(|(_, v)| v.clone())?;
        Some((data, size))
    }

    /// A paragraph (w:p) into blocks; `tab`: a table row's tab stop and
    /// cells already collected.
    fn paragraph(&mut self, p: Node) {
        let ppr = child(p, "pPr");
        let style = ppr
            .and_then(|n| child(n, "pStyle"))
            .and_then(|n| attr(n, "val"));
        let mut props = self.styles.of(style);
        if let Some(ppr) = ppr {
            props = props.with(&para_props(ppr));
            if let Some(rpr) = child(ppr, "rPr") {
                // Paragraph-mark properties do not apply to the text.
                let _ = rpr;
            }
        }
        let list = ppr.and_then(|n| child(n, "numPr")).is_some();
        let level = ppr
            .and_then(|n| child(n, "numPr"))
            .and_then(|n| child(n, "ilvl"))
            .and_then(|n| number(n, "val"))
            .unwrap_or(0.0);
        let mut para = self.new_para(&props);
        para.page_break_before = ppr
            .and_then(|n| child(n, "pageBreakBefore"))
            .is_some_and(toggle);
        if list {
            para.left = props.left.unwrap_or(0.0).max(24.0 * (level + 1.0));
            self.push_text(&mut para, &props, "•\u{2002}");
        }
        let mut images = Vec::new();
        self.runs(p, &props, &mut para, &mut images);
        self.blocks.push(Block::Para(para));
        for (data, size) in images {
            self.blocks.push(Block::Image {
                data,
                size,
                align: props.align.unwrap_or_default(),
            });
        }
    }

    fn new_para(&self, props: &Props) -> Para {
        Para {
            align: props.align.unwrap_or_default(),
            before: props.before.unwrap_or(0.0),
            after: props.after.unwrap_or(0.0),
            line: props.line.unwrap_or(1.0).clamp(0.6, 4.0),
            left: props.left.unwrap_or(0.0).max(0.0),
            right: props.right.unwrap_or(0.0).max(0.0),
            ..Para::default()
        }
    }

    fn push_text(&self, para: &mut Para, props: &Props, text: &str) {
        let start = para.text.len() as u32;
        para.text.extend(text.encode_utf16());
        let len = para.text.len() as u32 - start;
        if len == 0 {
            return;
        }
        para.runs.push(Run {
            start,
            len,
            bold: props.bold.unwrap_or(false),
            italic: props.italic.unwrap_or(false),
            underline: props.underline.unwrap_or(false),
            strike: props.strike.unwrap_or(false),
            size: props.size.unwrap_or(half_points(22.0)),
            color: props.color.unwrap_or([0.0; 3]),
            font: props.font.clone().unwrap_or_else(|| "Calibri".into()),
        });
    }

    /// The runs inside `node` (hyperlinks, insertions and fields too).
    fn runs(
        &mut self,
        node: Node,
        props: &Props,
        para: &mut Para,
        images: &mut Vec<(Vec<u8>, (f32, f32))>,
    ) {
        for c in node.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "r" => {
                    let rp = match child(c, "rPr") {
                        Some(rpr) => props.with(&run_props(rpr)),
                        None => props.clone(),
                    };
                    for e in c.children().filter(Node::is_element) {
                        match e.tag_name().name() {
                            "t" => self.push_text(para, &rp, e.text().unwrap_or("")),
                            "tab" => self.push_text(para, &rp, "\t"),
                            "br" if attr(e, "type") == Some("page") => {
                                // The rest of the paragraph starts a new page.
                                let mut next = self.new_para(props);
                                next.page_break_before = true;
                                next.before = 0.0;
                                let done = std::mem::replace(para, next);
                                self.blocks.push(Block::Para(done));
                            }
                            "br" | "cr" => self.push_text(para, &rp, "\u{2028}"),
                            "drawing" | "pict" => {
                                if let Some(img) = self.image(e) {
                                    images.push(img);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "hyperlink" | "ins" | "smartTag" | "fldSimple" | "customXml" => {
                    self.runs(c, props, para, images);
                }
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.runs(content, props, para, images);
                    }
                }
                _ => {}
            }
        }
    }

    /// A table: each row a paragraph with its cells at equal tab stops.
    fn table(&mut self, tbl: Node) {
        for tr in tbl.children().filter(|n| n.has_tag_name("tr")) {
            let cells: Vec<Node> = tr.children().filter(|n| n.has_tag_name("tc")).collect();
            if cells.is_empty() {
                continue;
            }
            let first = cells[0]
                .descendants()
                .find(|n| n.has_tag_name("p"))
                .and_then(|p| child(p, "pPr"))
                .and_then(|n| child(n, "pStyle"))
                .and_then(|n| attr(n, "val"));
            let props = self.styles.of(first);
            let mut row = self.new_para(&props);
            row.align = Align::Left;
            row.before = 2.0;
            row.after = 2.0;
            row.tab = Some(self.content_width / cells.len() as f32);
            let mut images = Vec::new();
            for (k, cell) in cells.iter().enumerate() {
                if k > 0 {
                    self.push_text(&mut row, &props, "\t");
                }
                for (j, p) in cell.children().filter(|n| n.has_tag_name("p")).enumerate() {
                    if j > 0 {
                        self.push_text(&mut row, &props, " ");
                    }
                    let pp = match child(p, "pPr") {
                        Some(ppr) => props.with(&para_props(ppr)),
                        None => props.clone(),
                    };
                    self.runs(p, &pp, &mut row, &mut images);
                }
            }
            self.blocks.push(Block::Para(row));
        }
    }
}

impl DocxDoc {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let data = std::fs::read(path)?;
        let bad = || DecodeError::Other("DOCX".into());
        let files = crate::model::unzip(&data, |_| true, 256 << 20).map_err(|_| bad())?;
        let text = |name: &str| {
            files
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        };
        let body_xml = text("word/document.xml").ok_or_else(bad)?;
        let styles = Styles::parse(text("word/styles.xml").as_deref());
        let rels: HashMap<String, String> = text("word/_rels/document.xml.rels")
            .and_then(|x| {
                let doc = Document::parse(&x).ok()?;
                Some(
                    doc.descendants()
                        .filter(|n| n.has_tag_name("Relationship"))
                        .filter_map(|n| {
                            Some((attr(n, "Id")?.to_string(), attr(n, "Target")?.to_string()))
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        let doc = Document::parse(&body_xml).map_err(|e| DecodeError::Other(e.to_string()))?;
        let body = doc
            .descendants()
            .find(|n| n.has_tag_name("body"))
            .ok_or_else(bad)?;
        // The page: the body's last section (A4 with Word's margins if none).
        let sect = body.children().rfind(|n| n.has_tag_name("sectPr"));
        let size = sect.and_then(|s| child(s, "pgSz"));
        let page = (
            size.and_then(|n| number(n, "w")).map_or(794.0, twips),
            size.and_then(|n| number(n, "h")).map_or(1123.0, twips),
        );
        let mar = sect.and_then(|s| child(s, "pgMar"));
        let m = |k: &str, d: f32| mar.and_then(|n| number(n, k)).map_or(d, twips);
        let margin = [
            m("left", 113.0),
            m("top", 76.0),
            m("right", 57.0),
            m("bottom", 76.0),
        ];
        let mut reader = Reader {
            styles: &styles,
            rels: &rels,
            files: &files,
            blocks: Vec::new(),
            content_width: (page.0 - margin[0] - margin[2]).max(40.0),
        };
        for c in body.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "p" => reader.paragraph(c),
                "tbl" => reader.table(c),
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        for p in content.children().filter(|n| n.has_tag_name("p")) {
                            reader.paragraph(p);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut doc = DocxDoc {
            page,
            margin,
            blocks: reader.blocks,
            pages: Vec::new(),
        };
        doc.paginate(&Ctx::new()?)?;
        Ok(doc)
    }

    fn content(&self) -> (f32, f32) {
        (
            (self.page.0 - self.margin[0] - self.margin[2]).max(40.0),
            (self.page.1 - self.margin[1] - self.margin[3]).max(40.0),
        )
    }

    /// The paragraph laid out at its width (colours are set when drawing).
    fn layout(&self, ctx: &Ctx, p: &Para) -> Result<IDWriteTextLayout, DecodeError> {
        let width = (self.content().0 - p.left - p.right).max(20.0);
        let first = p.runs.first();
        let size = first.map_or(14.0, |r| r.size);
        let font = HSTRING::from(first.map_or("Calibri", |r| r.font.as_str()));
        // SAFETY: plain DirectWrite calls; the text outlives them (the
        // layout copies it).
        unsafe {
            let f = ctx
                .dwrite
                .CreateTextFormat(
                    &font,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    windows::core::w!("ru-ru"),
                )
                .map_err(err)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP).map_err(err)?;
            let l = ctx
                .dwrite
                .CreateTextLayout(&p.text, &f, width, f32::MAX)
                .map_err(err)?;
            l.SetTextAlignment(match p.align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                Align::Justify => DWRITE_TEXT_ALIGNMENT_JUSTIFIED,
            })
            .map_err(err)?;
            // Line spacing as a multiple of the font's (Windows 10+).
            let _ = l.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_PROPORTIONAL, p.line, 0.8);
            if let Some(t) = p.tab {
                let _ = l.SetIncrementalTabStop(t.max(1.0));
            }
            for r in &p.runs {
                let range = DWRITE_TEXT_RANGE {
                    startPosition: r.start,
                    length: r.len,
                };
                let _ = l.SetFontFamilyName(&HSTRING::from(r.font.as_str()), range);
                let _ = l.SetFontSize(r.size, range);
                if r.bold {
                    let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
                }
                if r.italic {
                    let _ = l.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
                }
                if r.underline {
                    let _ = l.SetUnderline(true, range);
                }
                if r.strike {
                    let _ = l.SetStrikethrough(true, range);
                }
            }
            Ok(l)
        }
    }

    /// Heights of a paragraph's lines.
    fn line_heights(&self, ctx: &Ctx, p: &Para) -> Result<Vec<f32>, DecodeError> {
        let l = self.layout(ctx, p)?;
        let mut n = 0;
        // SAFETY: the first call reports the count, the second fills it.
        unsafe {
            let _ = l.GetLineMetrics(None, &mut n);
            let mut v = vec![DWRITE_LINE_METRICS::default(); n as usize];
            l.GetLineMetrics(Some(&mut v), &mut n).map_err(err)?;
            Ok(v.iter().map(|m| m.height).collect())
        }
    }

    /// A picture's size on the page: no wider than the text area.
    fn image_size(&self, size: (f32, f32)) -> (f32, f32) {
        let (w, h) = self.content();
        let k = (w / size.0.max(1.0)).min(h / size.1.max(1.0)).min(1.0);
        (size.0 * k, size.1 * k)
    }

    fn paginate(&mut self, ctx: &Ctx) -> Result<(), DecodeError> {
        let room = self.content().1;
        let mut pages: Vec<Vec<Piece>> = vec![Vec::new()];
        let mut y = 0.0f32;
        for (i, b) in self.blocks.iter().enumerate() {
            match b {
                Block::Para(p) => {
                    let page = pages.last_mut().expect("a page");
                    if p.page_break_before && !page.is_empty() {
                        pages.push(Vec::new());
                        y = 0.0;
                    }
                    if y > 0.0 {
                        y += p.before;
                    }
                    let lines = self.line_heights(ctx, p)?;
                    let (mut from, mut taken) = (0.0f32, 0.0f32);
                    for h in lines {
                        if y + taken + h > room && (taken > 0.0 || y > 0.0) {
                            if taken > 0.0 {
                                pages.last_mut().expect("a page").push(Piece {
                                    block: i,
                                    y,
                                    from,
                                    height: taken,
                                });
                            }
                            pages.push(Vec::new());
                            from += taken;
                            taken = 0.0;
                            y = 0.0;
                        }
                        taken += h;
                    }
                    pages.last_mut().expect("a page").push(Piece {
                        block: i,
                        y,
                        from,
                        height: taken,
                    });
                    y += taken + p.after;
                }
                Block::Image { size, .. } => {
                    let (_, h) = self.image_size(*size);
                    if y + h > room && y > 0.0 {
                        pages.push(Vec::new());
                        y = 0.0;
                    }
                    pages.last_mut().expect("a page").push(Piece {
                        block: i,
                        y,
                        from: 0.0,
                        height: h,
                    });
                    y += h + 6.0;
                }
            }
        }
        self.pages = pages;
        Ok(())
    }

    pub fn pages(&self) -> u32 {
        self.pages.len() as u32
    }

    /// The page size, DIPs.
    pub fn page_size(&self) -> (f64, f64) {
        (f64::from(self.page.0), f64::from(self.page.1))
    }

    /// Renders `region` (x0, y0, x1, y1 in pixels of a `raster`-sized
    /// rendering of the whole page) into `out` pixels, on white.
    pub fn render(
        &self,
        page: u32,
        raster: (u32, u32),
        region: [f64; 4],
        out: (u32, u32),
    ) -> Result<RawImage, DecodeError> {
        let ctx = Ctx::new()?;
        let base = f64::from(raster.0) / f64::from(self.page.0);
        let k = f64::from(out.0) / (region[2] - region[0]) * base;
        let (ox, oy) = (region[0] / base, region[1] / base);
        let pieces = self.pages.get(page as usize).cloned().unwrap_or_default();
        // SAFETY: COM objects of this thread; the bitmap outlives the
        // render target; the buffer is the size of the copy.
        unsafe {
            let bmp = ctx
                .wic
                .CreateBitmap(
                    out.0,
                    out.1,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
                .map_err(err)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            };
            let rt = ctx
                .d2d
                .CreateWicBitmapRenderTarget(&bmp, &props)
                .map_err(err)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }));
            let (k, ox, oy) = (k as f32, ox as f32, oy as f32);
            rt.SetTransform(&Matrix3x2 {
                M11: k,
                M12: 0.0,
                M21: 0.0,
                M22: k,
                M31: -ox * k,
                M32: -oy * k,
            });
            for piece in pieces {
                // Pictures that fail to decode are left out.
                let _ = self.draw_piece(&ctx, &rt, piece);
            }
            rt.EndDraw(None, None).map_err(err)?;
            let stride = out.0 * 4;
            let mut px = vec![0u8; (stride * out.1) as usize];
            bmp.CopyPixels(std::ptr::null(), stride, &mut px)
                .map_err(err)?;
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
                p[3] = 255;
            }
            Ok(RawImage::new(out.0, out.1, Raw::Rgba8(px)))
        }
    }

    fn draw_piece(
        &self,
        ctx: &Ctx,
        rt: &ID2D1RenderTarget,
        piece: Piece,
    ) -> Result<(), DecodeError> {
        let (cw, _) = self.content();
        let (x0, y0) = (self.margin[0], self.margin[1] + piece.y);
        match &self.blocks[piece.block] {
            Block::Para(p) => {
                let l = self.layout(ctx, p)?;
                // SAFETY: drawing inside BeginDraw / EndDraw of `rt`.
                unsafe {
                    // Run colours as drawing effects (black stays default).
                    for r in p.runs.iter().filter(|r| r.color != [0.0; 3]) {
                        let [cr, cg, cb] = r.color;
                        let brush = rt
                            .CreateSolidColorBrush(
                                &D2D1_COLOR_F {
                                    r: cr,
                                    g: cg,
                                    b: cb,
                                    a: 1.0,
                                },
                                None,
                            )
                            .map_err(err)?;
                        let unknown: windows::core::IUnknown = brush.cast().map_err(err)?;
                        let _ = l.SetDrawingEffect(
                            &unknown,
                            DWRITE_TEXT_RANGE {
                                startPosition: r.start,
                                length: r.len,
                            },
                        );
                    }
                    let black = rt
                        .CreateSolidColorBrush(
                            &D2D1_COLOR_F {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 1.0,
                            },
                            None,
                        )
                        .map_err(err)?;
                    rt.PushAxisAlignedClip(
                        &D2D_RECT_F {
                            left: x0 - 2.0,
                            top: y0,
                            right: x0 + cw + 2.0,
                            bottom: y0 + piece.height + 1.0,
                        },
                        D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                    rt.DrawTextLayout(
                        Vector2 {
                            X: x0 + p.left,
                            Y: y0 - piece.from,
                        },
                        &l,
                        &black,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                    rt.PopAxisAlignedClip();
                }
            }
            Block::Image { data, size, align } => {
                let (w, h) = self.image_size(*size);
                let x = match align {
                    Align::Center => x0 + (cw - w) / 2.0,
                    Align::Right => x0 + cw - w,
                    _ => x0,
                };
                // SAFETY: plain WIC / Direct2D calls on this thread's
                // objects; `data` outlives the stream.
                unsafe {
                    let stream = ctx.wic.CreateStream().map_err(err)?;
                    stream.InitializeFromMemory(data).map_err(err)?;
                    let decoder = ctx
                        .wic
                        .CreateDecoderFromStream(
                            &stream,
                            std::ptr::null(),
                            WICDecodeMetadataCacheOnDemand,
                        )
                        .map_err(err)?;
                    let frame = decoder.GetFrame(0).map_err(err)?;
                    let conv = ctx.wic.CreateFormatConverter().map_err(err)?;
                    conv.Initialize(
                        &frame,
                        &GUID_WICPixelFormat32bppPBGRA,
                        WICBitmapDitherTypeNone,
                        None,
                        0.0,
                        WICBitmapPaletteTypeMedianCut,
                    )
                    .map_err(err)?;
                    let bitmap = rt.CreateBitmapFromWicBitmap(&conv, None).map_err(err)?;
                    rt.DrawBitmap(
                        &bitmap,
                        Some(&D2D_RECT_F {
                            left: x,
                            top: y0,
                            right: x + w,
                            bottom: y0 + h,
                        }),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                }
            }
        }
        Ok(())
    }
}

/// The first page, fitted to the screen box; with the page count.
pub fn decode(path: &Path, hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let doc = DocxDoc::open(path)?;
    let raster = super::pdf::raster_size(doc.page_size(), hint);
    let region = [0.0, 0.0, f64::from(raster.0), f64::from(raster.1)];
    let mut img = doc.render(0, raster, region, raster)?;
    img.pages = doc.pages();
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dark (text) pixels in an RGBA8 rendering.
    fn ink(img: &RawImage) -> usize {
        let super::super::Raw::Rgba8(v) = &img.data else {
            panic!("not RGBA8");
        };
        v.as_chunks::<4>().0.iter().filter(|p| p[0] < 128).count()
    }

    /// The generated sample: A5 landscape from its section, several pages,
    /// text on the first and on the last (after a page break).
    #[test]
    fn pages_in_the_file_format() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.docx");
        if !p.exists() {
            return;
        }
        let doc = DocxDoc::open(&p).expect("open");
        let (w, h) = doc.page_size();
        assert!(
            (w - 793.7).abs() < 1.0 && (h - 559.4).abs() < 1.0,
            "{w} × {h}"
        );
        assert!(doc.pages() >= 3, "{} pages", doc.pages());
        let raster = (794, 559);
        let all = [0.0, 0.0, 794.0, 559.0];
        for page in [0, doc.pages() - 1] {
            let img = doc.render(page, raster, all, raster).expect("render");
            assert_eq!((img.width, img.height), raster);
            assert!(ink(&img) > 200, "page {page}: {} dark pixels", ink(&img));
        }
    }
}
