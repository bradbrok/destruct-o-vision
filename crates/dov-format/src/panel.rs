//! Panel art: an SVG that restyles a module's panel.
//!
//! - **Background art** has no `components` group. It is stretched over the
//!   panel, and the controls, labels and title are laid out automatically on
//!   top, so any module can be restyled without moving anything.
//! - **Full art** has a group with id `components` whose shapes mark where
//!   the controls go, each named for what it places (see [`component_id`]).
//!   The art then owns the panel: its width sets the panel's, it draws its own
//!   labels, and the `components` group itself isn't drawn.
//!
//! Panels are 380 units tall and 15 units per HP; art of any size is scaled
//! to that height. Art may embed images as data URLs but can't reference
//! files, and text in it is drawn with the system's fonts.

use std::sync::Arc;

use crate::interface::Interface;
use crate::module::Error;

/// Panel height in panel units (pixels at zoom 1).
pub const PANEL_HEIGHT: f32 = 380.0;
/// Width of one HP in panel units.
pub const HP: f32 = 15.0;
/// Art files bigger than this are refused unread.
pub const MAX_ART_BYTES: u64 = 2 << 20;
/// Elements in the file, and in the tree once `<use>` references are
/// expanded (which can grow exponentially).
const MAX_ELEMENTS: usize = 20_000;
const MAX_EXPANDED: u64 = 100_000;
/// Largest embedded image, in pixels per side.
const MAX_IMAGE_SIDE: u32 = 4096;
/// Id of the group that marks control positions.
pub const COMPONENTS: &str = "components";

/// Parsed panel art, ready to draw.
pub struct PanelArt {
    /// The art without its `components` group.
    pub tree: usvg::Tree,
    /// Width in panel units, at the panel's height.
    pub width: f32,
    /// For full art: where it places each control.
    pub components: Option<Vec<Component>>,
}

/// A shape in the `components` group: what it places, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub id: String,
    /// Bounding box in panel units: left, top, right, bottom.
    pub rect: [f32; 4],
}

impl Component {
    pub fn center(&self) -> [f32; 2] {
        [
            (self.rect[0] + self.rect[2]) / 2.0,
            (self.rect[1] + self.rect[3]) / 2.0,
        ]
    }
}

impl PanelArt {
    /// Width in whole HP.
    pub fn hp(&self) -> i32 {
        ((self.width / HP).round() as i32).max(2)
    }
}

/// What a control's shape is named in the `components` group. The control
/// is centered on the shape's bounding box.
pub enum Place<'a> {
    /// A knob or switch.
    Param(&'a str),
    /// The CV jack of a param. Its attenuverter knob sits 26 units to the
    /// right of the jack.
    Cv(&'a str),
    Input(&'a str),
    Output(&'a str),
    /// The text field of modules with one (a rectangle).
    Text,
    /// The live picture of modules with a screen (a rectangle).
    Screen,
}

pub fn component_id(place: Place<'_>) -> String {
    match place {
        Place::Param(id) => format!("param-{id}"),
        Place::Cv(id) => format!("cv-{id}"),
        Place::Input(id) => format!("in-{id}"),
        Place::Output(id) => format!("out-{id}"),
        Place::Text => "text".into(),
        Place::Screen => "screen".into(),
    }
}

/// Every component full art must place for a module: each param, CV jack,
/// visible input and output, plus the text field and screen if it has them.
pub fn required_components(iface: &Interface, screen: bool) -> Vec<String> {
    let mut ids = Vec::new();
    if iface.text {
        ids.push(component_id(Place::Text));
    }
    if screen {
        ids.push(component_id(Place::Screen));
    }
    for p in &iface.params {
        ids.push(component_id(Place::Param(&p.id)));
    }
    for i in iface.cv_params() {
        ids.push(component_id(Place::Cv(&iface.params[i].id)));
    }
    ids.extend(
        iface
            .inputs
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| component_id(Place::Input(&p.id))),
    );
    ids.extend(
        iface
            .outputs
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| component_id(Place::Output(&p.id))),
    );
    ids
}

/// Checks that full art places exactly the components `required`.
pub fn check_components(components: &[Component], required: &[String]) -> Result<(), Error> {
    let mut messages = Vec::new();
    for id in required {
        if !components.iter().any(|c| &c.id == id) {
            messages.push(format!("the art doesn't place `{id}`"));
        }
    }
    for c in components {
        if !required.contains(&c.id) {
            messages.push(format!(
                "the art places `{}`, which this module doesn't have",
                c.id
            ));
        }
    }
    if messages.is_empty() {
        Ok(())
    } else {
        Err(Error { messages })
    }
}

fn options(fontdb: Arc<usvg::fontdb::Database>) -> usvg::Options<'static> {
    let mut opt = usvg::Options {
        // Embedded images only: art never reads other files.
        resources_dir: None,
        fontdb,
        ..Default::default()
    };
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(|_mime, data, _| safe_image(data)),
        resolve_string: Box::new(|_, _| None),
    };
    opt
}

/// Embedded PNG, JPEG and GIF images whose headers declare a sane size.
/// Everything else is dropped: nested SVG (parsed with fewer limits) and
/// WebP, and any image that would decode into a huge bitmap.
fn safe_image(data: Arc<Vec<u8>>) -> Option<usvg::ImageKind> {
    let ok = |(w, h): (u32, u32)| w <= MAX_IMAGE_SIDE && h <= MAX_IMAGE_SIDE;
    let be16 = |i: usize| Some(u32::from(u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?])));
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(data.get(16..20)?.try_into().ok()?);
        let h = u32::from_be_bytes(data.get(20..24)?.try_into().ok()?);
        return ok((w, h)).then(|| usvg::ImageKind::PNG(data.clone()));
    }
    if data.starts_with(b"GIF8") {
        let w = u32::from(u16::from_le_bytes(data.get(6..8)?.try_into().ok()?));
        let h = u32::from(u16::from_le_bytes(data.get(8..10)?.try_into().ok()?));
        return ok((w, h)).then(|| usvg::ImageKind::GIF(data.clone()));
    }
    if data.starts_with(&[0xff, 0xd8]) {
        // Walk the JPEG segments to the frame header (SOFn).
        let mut i = 2;
        while *data.get(i)? == 0xff {
            let marker = *data.get(i + 1)?;
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                let (h, w) = (be16(i + 5)?, be16(i + 7)?);
                return ok((w, h)).then(|| usvg::ImageKind::JPEG(data.clone()));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

/// Limits on the art's structure, checked before usvg sees it: no DTD (no
/// entity tricks), a bounded number of elements, no markers (copied onto
/// every vertex of a path), no filter settings that multiply the work per
/// pixel, and a bound on the work references create.
///
/// References are counted conservatively rather than by mirroring usvg's
/// rules: every `href` and every `url(#id)` in any attribute adds the size
/// of what it names, ids must be unique, a `<use>` must resolve, and
/// stylesheets (whose selectors could attach references to any number of
/// elements) can't contain `url(`.
fn check_structure(doc: &roxmltree::Document) -> Result<(), String> {
    use std::collections::HashMap;

    let elements: Vec<roxmltree::Node> = doc.descendants().filter(|n| n.is_element()).collect();
    if elements.len() > MAX_ELEMENTS {
        return Err(format!(
            "{} elements is too many (at most {MAX_ELEMENTS})",
            elements.len()
        ));
    }
    let mut by_id: HashMap<&str, roxmltree::Node> = HashMap::new();
    for n in &elements {
        let tag = n.tag_name().name();
        if tag == "marker" {
            return Err("markers aren't supported in panel art".into());
        }
        // Its cost grows with the radius squared at every pixel, and art
        // scaled up enlarges the radius: no cap on the number is safe.
        if tag == "feMorphology" {
            return Err("feMorphology isn't supported in panel art".into());
        }
        // CSS could hide references behind escapes, imports or selectors.
        let css: String = match tag {
            "style" => n.descendants().filter_map(|t| t.text()).collect(),
            _ => n.attribute("style").unwrap_or_default().to_string(),
        };
        let lower = css.to_ascii_lowercase();
        if css.contains('\\') || lower.contains("@import") || (tag == "style" && lower.contains("url(")) {
            return Err(
                "CSS in panel art can't use escapes, @import, or url() in stylesheets; \
                 put references on the elements"
                    .into(),
            );
        }
        let too_big = |attr: &str, max: f32| {
            n.attribute(attr)
                .and_then(|v| v.split_whitespace().next()?.parse::<f32>().ok())
                .is_some_and(|v| v.is_nan() || v.abs() > max)
        };
        if (tag == "feTurbulence" && too_big("numOctaves", 8.0))
            || (tag == "feConvolveMatrix" && too_big("order", 9.0))
        {
            return Err(format!("`{tag}` settings are too heavy for panel art"));
        }
        if let Some(id) = n.attribute("id")
            && by_id.insert(id.trim(), *n).is_some()
        {
            return Err(format!("the id `{id}` is used more than once"));
        }
    }

    /// Ids named by `url(...)` anywhere in a value, leniently parsed (any
    /// case, spaces and quotes allowed), so nothing usvg reads is missed.
    fn url_refs(value: &str) -> Vec<&str> {
        value
            .to_ascii_lowercase()
            .match_indices("url(")
            .filter_map(|(i, _)| {
                let inner = value[i + 4..]
                    .split(')')
                    .next()?
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim();
                inner.strip_prefix('#')
            })
            .collect()
    }

    /// The work an element amounts to: itself, its children, and everything
    /// it references, counted again for every reference.
    fn work<'a>(
        n: roxmltree::Node<'a, 'a>,
        by_id: &HashMap<&str, roxmltree::Node<'a, 'a>>,
        memo: &mut HashMap<roxmltree::NodeId, u64>,
        depth: u32,
    ) -> Result<u64, String> {
        if depth > 64 {
            return Err("references nest too deeply (or loop)".into());
        }
        if let Some(&size) = memo.get(&n.id()) {
            return Ok(size);
        }
        let mut size = 1u64;
        for child in n.children().filter(|c| c.is_element()) {
            size = size.saturating_add(work(child, by_id, memo, depth + 1)?);
        }
        let tag = n.tag_name().name();
        for attr in n.attributes() {
            let value = attr.value();
            let mut targets: Vec<&str> = url_refs(value);
            if attr.name() == "href" {
                match value.trim().strip_prefix('#') {
                    Some(id) => targets.push(id),
                    None if tag == "use" => {
                        return Err("`<use>` can only name an element in the same file".into());
                    }
                    None => {}
                }
            }
            for id in targets {
                match by_id.get(id.trim()) {
                    Some(target) => size = size.saturating_add(work(*target, by_id, memo, depth + 1)?),
                    None if tag == "use" => return Err(format!("`<use>` names `{id}`, which doesn't exist")),
                    None => {}
                }
            }
        }
        memo.insert(n.id(), size);
        Ok(size)
    }
    let size = work(doc.root_element(), &by_id, &mut HashMap::new(), 0)?;
    if size > MAX_EXPANDED {
        return Err(format!(
            "references expand the art to {size} elements' worth of drawing (at most {MAX_EXPANDED})"
        ));
    }
    Ok(())
}

/// Parses panel art. `fontdb` supplies fonts for any text in it.
pub fn parse_art(svg: &str, fontdb: Arc<usvg::fontdb::Database>) -> Result<PanelArt, Error> {
    let fail = |m: String| Error { messages: vec![m] };
    if svg.len() as u64 > MAX_ART_BYTES {
        return Err(fail(format!("{} bytes is too big for panel art", svg.len())));
    }
    // roxmltree's defaults refuse a DTD, so entity tricks never reach usvg
    // (which would allow them).
    let doc = roxmltree::Document::parse(svg).map_err(|e| fail(format!("not valid SVG: {e}")))?;
    check_structure(&doc).map_err(fail)?;
    let group = doc.descendants().find(|n| n.attribute("id") == Some(COMPONENTS));
    let opt = options(fontdb);

    // Positions come from the art as authored; the drawing leaves the
    // components group out.
    let components = match group {
        Some(_) => {
            let full = usvg::Tree::from_str(svg, &opt).map_err(|e| fail(format!("not valid SVG: {e}")))?;
            let scale = PANEL_HEIGHT / full.size().height();
            let mut found = Vec::new();
            if let Some(usvg::Node::Group(g)) = full.node_by_id(COMPONENTS) {
                collect(g, scale, &mut found);
            }
            Some(found)
        }
        None => None,
    };
    let drawn = match group {
        Some(g) => {
            let range = g.range();
            format!("{}{}", &svg[..range.start], &svg[range.end..])
        }
        None => svg.to_string(),
    };
    let tree = usvg::Tree::from_str(&drawn, &opt).map_err(|e| fail(format!("not valid SVG: {e}")))?;
    let size = tree.size();
    if !(size.width() > 0.0 && size.height() > 0.0) {
        return Err(fail("the art has no size".into()));
    }
    let width = size.width() * PANEL_HEIGHT / size.height();
    if !(2.0 * HP..=64.0 * HP).contains(&width) {
        return Err(fail(format!(
            "at the panel's height the art is {:.0} wide; panels are 2 to 64 HP ({} to {} units)",
            width,
            2.0 * HP,
            64.0 * HP
        )));
    }
    Ok(PanelArt {
        tree,
        width,
        components,
    })
}

/// Named shapes anywhere inside the components group.
fn collect(group: &usvg::Group, scale: f32, out: &mut Vec<Component>) {
    for node in group.children() {
        let id = node.id();
        if !id.is_empty() && !matches!(node, usvg::Node::Group(_)) {
            let b = node.abs_bounding_box();
            out.push(Component {
                id: id.to_string(),
                rect: [
                    b.left() * scale,
                    b.top() * scale,
                    b.right() * scale,
                    b.bottom() * scale,
                ],
            });
        }
        if let usvg::Node::Group(g) = node {
            if !id.is_empty() && g.children().len() == 1 {
                // A named group around a single shape (common in Figma and
                // Illustrator exports) places by the group's name.
                let b = node.abs_bounding_box();
                out.push(Component {
                    id: id.to_string(),
                    rect: [
                        b.left() * scale,
                        b.top() * scale,
                        b.right() * scale,
                        b.bottom() * scale,
                    ],
                });
            } else {
                collect(g, scale, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Arc<usvg::fontdb::Database> {
        Arc::new(usvg::fontdb::Database::new())
    }

    const FULL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190">
  <rect width="60" height="190" fill="#203040"/>
  <g id="components">
    <circle id="param-gain" cx="30" cy="40" r="10" fill="red"/>
    <rect id="in-sig" x="10" y="140" width="10" height="10"/>
    <g id="out-out"><circle cx="45" cy="145" r="5"/></g>
  </g>
</svg>"##;

    #[test]
    fn full_art_places_components_scaled_to_the_panel() {
        let art = parse_art(FULL, fonts()).unwrap();
        assert_eq!(art.width, 120.0, "190 tall scales by 2 to 380");
        assert_eq!(art.hp(), 8);
        let c = art.components.unwrap();
        let find = |id: &str| c.iter().find(|c| c.id == id).unwrap().center();
        assert_eq!(find("param-gain"), [60.0, 80.0]);
        assert_eq!(find("in-sig"), [30.0, 290.0]);
        assert_eq!(find("out-out"), [90.0, 290.0]);
        assert!(
            art.tree.node_by_id(COMPONENTS).is_none(),
            "the components aren't drawn"
        );
    }

    #[test]
    fn background_art_has_no_components() {
        let art = parse_art(&FULL.replace("id=\"components\"", "id=\"decor\""), fonts()).unwrap();
        assert!(art.components.is_none());
    }

    #[test]
    fn components_must_match_the_module() {
        let art = parse_art(FULL, fonts()).unwrap();
        let required: Vec<String> = ["param-gain", "cv-gain", "in-sig"].map(String::from).to_vec();
        let e = check_components(&art.components.unwrap(), &required)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("doesn't place `cv-gain`") && e.contains("`out-out`, which"),
            "{e}"
        );
    }

    #[test]
    fn art_cant_blow_up() {
        let laughs = r#"<?xml version="1.0"?>
<!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]>
<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><text>&b;</text></svg>"#;
        assert!(parse_art(laughs, fonts()).is_err(), "DTDs are refused");

        // Each level uses the one below twice: 2^30 elements once expanded.
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="60" height="190"><defs><rect id="l0" width="1" height="1"/>"#,
        );
        for i in 1..=30 {
            svg += &format!(
                r##"<g id="l{i}"><use xlink:href="#l{}"/><use xlink:href="#l{}"/></g>"##,
                i - 1,
                i - 1
            );
        }
        svg += r##"</defs><use xlink:href="#l30"/></svg>"##;
        let e = parse_art(&svg, fonts()).err().unwrap().to_string();
        assert!(e.contains("expand"), "{e}");

        // Ways around a naive count: duplicate ids, padded references, and
        // references that aren't <use>.
        let dup = svg.replacen(r#"<rect id="l0""#, r#"<rect id="l0"/><rect id="l0""#, 1);
        assert!(
            parse_art(&dup, fonts())
                .err()
                .unwrap()
                .to_string()
                .contains("more than once")
        );
        let padded = svg.replace(r##"href="#l"##, r##"href=" #l"##);
        assert!(
            parse_art(&padded, fonts())
                .err()
                .unwrap()
                .to_string()
                .contains("expand")
        );
        let mut masks = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><defs><mask id="m0"><rect width="1" height="1"/></mask>"#,
        );
        for i in 1..=30 {
            masks += &format!(
                r#"<mask id="m{i}"><rect width="1" height="1" mask="url( #m{} )"/><rect width="1" height="1" mask="url(#m{})"/></mask>"#,
                i - 1,
                i - 1
            );
        }
        masks += r#"</defs><rect width="9" height="9" mask="url(#m30)"/></svg>"#;
        assert!(
            parse_art(&masks, fonts())
                .err()
                .unwrap()
                .to_string()
                .contains("expand")
        );
        let css = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><style>rect { mask: url(#m) }</style></svg>"#;
        assert!(
            parse_art(css, fonts())
                .err()
                .unwrap()
                .to_string()
                .contains("stylesheets")
        );
        let dangling = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><use href="#nowhere"/></svg>"##;
        assert!(parse_art(dangling, fonts()).is_err());
        let error = |svg: &str| parse_art(svg, fonts()).err().unwrap().to_string();
        assert!(
            error(&masks.replace("url(", "URL(")).contains("expand"),
            "any case counts"
        );
        let escaped = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><rect width="9" height="9" style="mask: u\72 l(#m)"/></svg>"#;
        assert!(error(escaped).contains("escapes"));
        let morph = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="190"><filter id="f"><feMorphology radius="9999"/></filter><rect width="9" height="9" filter="url(#f)"/></svg>"##;
        assert!(error(morph).contains("feMorphology"));

        // A tiny PNG claiming to be 100000 x 100000 pixels.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&100_000u32.to_be_bytes());
        png.extend_from_slice(&100_000u32.to_be_bytes());
        assert!(safe_image(Arc::new(png)).is_none());
    }

    #[test]
    fn art_cant_read_files() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="60" height="190">
  <image href="/etc/hosts" width="60" height="190"/>
  <image xlink:href="../../secret.png" width="60" height="190"/>
</svg>"##;
        let art = parse_art(svg, fonts()).unwrap();
        assert!(
            art.tree.root().children().is_empty(),
            "file references are dropped"
        );
    }
}
