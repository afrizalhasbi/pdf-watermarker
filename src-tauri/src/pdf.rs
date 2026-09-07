use base64::Engine;
use lopdf::{Document, Object, ObjectId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize)]
pub struct Meta {
    width: f64,
    height: f64,
    page_count: u32,
}

#[derive(Deserialize)]
pub struct TextItem {
    pub text: String,
    pub font_size: f64,
    pub x_frac: f64,
    pub y_frac: f64,
    #[serde(default)]
    pub align: Option<String>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn mediabox(doc: &Document, page_id: ObjectId) -> Result<Vec<f64>, String> {
    let box_of = |dict: &lopdf::Dictionary, key: &[u8]| -> Option<Vec<f64>> {
        let arr = dict.get(key).ok()?.as_array().ok()?;
        let v: Option<Vec<f64>> = arr
            .iter()
            .map(|o| o.as_float().ok().map(|f| f as f64))
            .collect();
        v.filter(|b| b.len() == 4)
    };
    let mut id = page_id;
    for _ in 0..16 {
        let dict = doc.get_dictionary(id).map_err(err)?;
        // viewers display the CropBox; prefer it, fall back to MediaBox
        if let Some(b) = box_of(dict, b"CropBox").or_else(|| box_of(dict, b"MediaBox")) {
            return Ok(b);
        }
        id = dict
            .get(b"Parent")
            .and_then(Object::as_reference)
            .map_err(err)?;
    }
    Err("no MediaBox found in page tree".into())
}

fn load(path: &str) -> Result<Document, String> {
    let doc = Document::load(PathBuf::from(path)).map_err(err)?;
    if doc.is_encrypted() {
        return Err("PDF is encrypted".into());
    }
    Ok(doc)
}

#[tauri::command]
pub fn load_pdf(path: String) -> Result<Meta, String> {
    let doc = load(&path)?;
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err("PDF has no pages".into());
    }
    let mut size: Option<(f64, f64)> = None;
    for page_id in pages.values() {
        let mb = mediabox(&doc, *page_id)?;
        if mb.len() != 4 {
            return Err("page MediaBox is malformed".into());
        }
        let (w, h) = ((mb[2] - mb[0]).abs(), (mb[3] - mb[1]).abs());
        if w <= 0.0 || h <= 0.0 {
            return Err("page has invalid size".into());
        }
        match size {
            None => size = Some((w, h)),
            Some((pw, ph)) if (pw - w).abs() > 0.5 || (ph - h).abs() > 0.5 => {
                return Err(format!(
                    "pages are not all the same size ({}x{}pt vs {}x{}pt)",
                    pw, ph, w, h
                ))
            }
            _ => {}
        }
    }
    let (width, height) = size.ok_or("PDF has no pages")?;
    Ok(Meta {
        width,
        height,
        page_count: pages.len() as u32,
    })
}

/// Extract one page into a standalone one-page PDF, returned as base64.
#[tauri::command]
pub fn preview_pdf(path: String, page_index: u32) -> Result<String, String> {
    let mut doc = load(&path)?;
    let pages = doc.get_pages();
    let page_id = *pages
        .get(&page_index)
        .ok_or_else(|| format!("page {} out of range", page_index))?;

    let box_arr: Vec<Object> = mediabox(&doc, page_id)?
        .into_iter()
        .map(|v| Object::Real(v as f32))
        .collect();

    let new_pages_id = doc.add_object(Object::Dictionary({
        let mut d = lopdf::Dictionary::new();
        d.set("Type", Object::Name(b"Pages".to_vec()));
        d.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        d.set("Count", Object::Integer(1));
        d.set("MediaBox", Object::Array(box_arr));
        d
    }));

    let page = doc.get_object_mut(page_id).map_err(err)?;
    let page_dict = Object::as_dict_mut(page).map_err(|_| "page object is not a dictionary")?;
    page_dict.set("Parent", Object::Reference(new_pages_id));

    let catalog_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(err)?;
    let catalog = doc.get_object_mut(catalog_id).map_err(err)?;
    let catalog_dict = Object::as_dict_mut(catalog).map_err(|_| "catalog is not a dictionary")?;
    catalog_dict.set("Pages", Object::Reference(new_pages_id));

    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(err)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// Whole source file as base64 (frontend pdf.js preview).
#[tauri::command]
pub fn pdf_bytes_b64(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(err)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Liberation Sans (metrically compatible with Arial, freely redistributable).
static STAMP_FONT: &[u8] = include_bytes!("../assets/GlacialIndifference-Regular.otf");

/// Render text at font_size pt (rasterized at RASTER_SCALE for crispness).
/// Returns RGB + separate alpha plane plus ink dimensions.
struct Stamp {
    rgb: Vec<u8>,
    alpha: Vec<u8>,
    w: usize,
    h: usize,
}

const RASTER_SCALE: f32 = 4.0;

/// Horizontal alignment of lines within the ink block.
#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    fn factor(self) -> f32 {
        match self {
            Align::Left => 0.0,
            Align::Center => 0.5,
            Align::Right => 1.0,
        }
    }
}

fn render_stamp(text: &str, font_size: f64, align: Align, r: u8, g: u8, b: u8, a: u8) -> Result<Stamp, String> {
    if text.is_empty() {
        return Err("watermark text is empty".into());
    }
    let px = (font_size as f32 * RASTER_SCALE).max(8.0);
    let font =
        fontdue::Font::from_bytes(STAMP_FONT, fontdue::FontSettings::default()).map_err(err)?;

    let metrics = font.horizontal_line_metrics(px);
    let line_h = metrics
        .map(|m| (m.ascent - m.descent).round() as i32)
        .unwrap_or((px * 1.2).round() as i32);
    let mut glyphs: Vec<(f32, i32, fontdue::Metrics, Vec<u8>)> = Vec::new();
    let mut line_widths: Vec<f32> = Vec::new();
    for (li, line) in text.split('\n').enumerate() {
        let baseline = -(line_h * li as i32); // up-positive raster space
        let mut pen = 0.0f32;
        for ch in line.chars() {
            let (m, bitmap) = font.rasterize(ch, px);
            glyphs.push((pen, baseline, m, bitmap));
            pen += m.advance_width;
        }
        line_widths.push(pen);
    }

    // alignment: shift each line so it sits left/center/right within the block
    let max_w = line_widths.iter().cloned().fold(0.0f32, f32::max);
    // lines are contiguous runs sharing the same baseline
    let mut shift_by_glyph: Vec<f32> = Vec::with_capacity(glyphs.len());
    {
        let mut li = 0usize;
        for (_p, bl, _, _) in &glyphs {
            let expected = -(line_h * li as i32);
            if *bl != expected {
                li += 1;
            }
            let w = line_widths.get(li).copied().unwrap_or(0.0);
            shift_by_glyph.push((max_w - w) * align.factor());
        }
    }
    for ((p, _, _, _), shift) in glyphs.iter_mut().zip(shift_by_glyph) {
        *p += shift;
    }

    // ink bbox in raster space (top-down, baseline at 0)
    let mut left = i32::MAX;
    let mut right = i32::MIN;
    let mut top = i32::MIN;
    let mut bottom = i32::MAX;
    for (p, bl, m, _) in &glyphs {
        left = left.min((*p) as i32 + m.xmin);
        right = right.max((*p) as i32 + m.xmin + m.width as i32);
        // fontdue: ymin = bitmap BOTTOM relative to baseline (up-positive);
        // bitmap row 0 is the top, at ymin + height
        top = top.max(bl + m.ymin + m.height as i32);
        bottom = bottom.min(bl + m.ymin);
    }
    let (w, h) = ((right - left) as usize, (top - bottom) as usize);
    if w == 0 || h == 0 || w > 16000 || h > 16000 {
        return Err("watermark text rasterized to an invalid bitmap".into());
    }

    let mut rgb = vec![0u8; w * h * 3];
    let mut alpha = vec![0u8; w * h];
    for (p, bl, m, bitmap) in &glyphs {
        let gx = (*p) as i32 + m.xmin - left;
        // canvas row 0 = ink top; glyph bitmap top sits at (ymin + height) above baseline
        let gy = top - (bl + m.ymin + m.height as i32);
        for row in 0..m.height {
            for col in 0..m.width {
                let cov = bitmap[row * m.width + col];
                if cov == 0 {
                    continue;
                }
                let x = gx + col as i32;
                let y = gy + row as i32;
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let idx = (y as usize) * w + x as usize;
                rgb[idx * 3] = r;
                rgb[idx * 3 + 1] = g;
                rgb[idx * 3 + 2] = b;
                alpha[idx] = ((cov as u16 * a as u16) / 255) as u8;
            }
        }
    }
    Ok(Stamp { rgb, alpha, w, h })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn apply_watermark(
    path: String,
    out_path: String,
    texts: Vec<TextItem>,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) -> Result<(), String> {
    let mut doc = load(&path)?;
    let pages = doc.get_pages();
    if pages.len() < 3 {
        return Err("PDF needs at least 3 pages (first and last are skipped)".into());
    }
    if texts.is_empty() {
        return Err("no watermark texts provided".into());
    }
    for t in &texts {
        if !(0.0..=1.0).contains(&t.x_frac) || !(0.0..=1.0).contains(&t.y_frac) {
            return Err("watermark position out of bounds".into());
        }
    }

    // render each text into its own image XObject
    let mut stamps: Vec<(f64, f64, ObjectId)> = Vec::new();
    for item in &texts {
        let align = match item.align.as_deref().unwrap_or("center") {
            "left" => Align::Left,
            "right" => Align::Right,
            _ => Align::Center,
        };
        let stamp = render_stamp(&item.text, item.font_size, align, r, g, b, a)?;
        let smask_id = doc.add_object(lopdf::Stream::new(
            {
                let mut d = lopdf::Dictionary::new();
                d.set("Type", Object::Name(b"XObject".to_vec()));
                d.set("Subtype", Object::Name(b"Image".to_vec()));
                d.set("Width", Object::Integer(stamp.w as i64));
                d.set("Height", Object::Integer(stamp.h as i64));
                d.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
                d.set("BitsPerComponent", Object::Integer(8));
                d
            },
            stamp.alpha.clone(),
        ));
        let image_id = doc.add_object(lopdf::Stream::new(
            {
                let mut d = lopdf::Dictionary::new();
                d.set("Type", Object::Name(b"XObject".to_vec()));
                d.set("Subtype", Object::Name(b"Image".to_vec()));
                d.set("Width", Object::Integer(stamp.w as i64));
                d.set("Height", Object::Integer(stamp.h as i64));
                d.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
                d.set("BitsPerComponent", Object::Integer(8));
                d.set("SMask", Object::Reference(smask_id));
                d
            },
            stamp.rgb.clone(),
        ));
        stamps.push((
            stamp.w as f64 / RASTER_SCALE as f64,
            stamp.h as f64 / RASTER_SCALE as f64,
            image_id,
        ));
    }

    // watermark every page except first and last
    for (_, page_id) in pages.iter().skip(1).take(pages.len().saturating_sub(2)) {
        let mb = mediabox(&doc, *page_id)?;
        let (w, h) = ((mb[2] - mb[0]).abs(), (mb[3] - mb[1]).abs());

        let mut resources_ops: Option<ObjectId> = None;
        let mut ops = String::new();
        for (i, item) in texts.iter().enumerate() {
            let (w_pt, h_pt, image_id) = stamps[i];
            // drag position = center of the stamped image
            let x = mb[0] + item.x_frac * w - w_pt / 2.0;
            let y = mb[1] + (1.0 - item.y_frac) * h - h_pt / 2.0;
            println!(
                "[wm] page: Box=({},{},{},{}) w={w} h={h} x_frac={} y_frac={} -> x={x} y={y} {w_pt}x{h_pt}pt",
                mb[0], mb[1], mb[2], mb[3], item.x_frac, item.y_frac
            );

            // ensure page resources have our XObject
            let resources = match resources_ops {
                Some(id) => id,
                None => {
                    let id = ensure_resources(&mut doc, *page_id)?;
                    resources_ops = Some(id);
                    id
                }
            };
            let xo_val = {
                let res = doc.get_object(resources).map_err(err)?;
                let d = Object::as_dict(res).map_err(|_| "resources not a dictionary")?;
                d.get(b"XObject").cloned().ok()
            };
            let xo_entry = match xo_val {
                Some(Object::Reference(id)) => id,
                Some(o @ Object::Dictionary(_)) => doc.add_object(o),
                _ => doc.add_object(Object::Dictionary(lopdf::Dictionary::new())),
            };
            let name = format!("Im{i}");
            let xo_obj = doc
                .get_object_mut(xo_entry)
                .and_then(Object::as_dict_mut)
                .map_err(err)?;
            xo_obj.set(name.as_bytes(), Object::Reference(image_id));
            // make sure the resources dict actually points at the XObject dict
            doc.get_object_mut(resources)
                .and_then(Object::as_dict_mut)
                .map_err(err)?
                .set("XObject", Object::Reference(xo_entry));

            ops.push_str(&format!("\nq {w_pt} 0 0 {h_pt} {x} {y} cm /{name} Do Q\n"));
        }
        doc.add_page_contents(*page_id, ops.into_bytes())
            .map_err(err)?;
    }

    doc.save(PathBuf::from(&out_path)).map_err(err)?;
    Ok(())
}

/// Resolve (or create) a Resources dictionary for a page; returns its object id.
fn ensure_resources(doc: &mut Document, page_id: ObjectId) -> Result<ObjectId, String> {
    let (res_id, existing) = {
        let page = doc.get_object_mut(page_id).map_err(err)?;
        let page_dict = Object::as_dict_mut(page).map_err(|_| "page not a dictionary")?;
        match page_dict.get(b"Resources") {
            Ok(Object::Reference(id)) => (Some(*id), None),
            Ok(res @ Object::Dictionary(_)) => (None, Some(res.clone())),
            _ => (None, None),
        }
    };
    let res_id = match (res_id, existing) {
        (Some(id), _) => id,
        (None, Some(res)) => {
            let id = doc.add_object(res);
            let page = doc.get_object_mut(page_id).map_err(err)?;
            Object::as_dict_mut(page)
                .map_err(|_| "page not a dictionary")?
                .set("Resources", Object::Reference(id));
            id
        }
        (None, None) => {
            let id = doc.add_object(Object::Dictionary(lopdf::Dictionary::new()));
            let page = doc.get_object_mut(page_id).map_err(err)?;
            Object::as_dict_mut(page)
                .map_err(|_| "page not a dictionary")?
                .set("Resources", Object::Reference(id));
            id
        }
    };
    Ok(res_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Stream;

    pub(crate) fn make_pdf(n_pages: u32, path: &str) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.add_object(Object::Dictionary({
            let mut d = lopdf::Dictionary::new();
            d.set("Type", Object::Name(b"Pages".to_vec()));
            d.set("Count", Object::Integer(n_pages as i64));
            d.set(
                "MediaBox",
                Object::Array(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(612),
                    Object::Integer(792),
                ]),
            );
            d
        }));
        let mut kids = Vec::new();
        for i in 0..n_pages {
            let content_id = doc.add_object(Object::Stream(Stream::new(
                lopdf::Dictionary::new(),
                format!("BT /F1 12 Tf 72 72 Td (page {i}) Tj ET").into_bytes(),
            )));
            let page_id = doc.add_object(Object::Dictionary({
                let mut d = lopdf::Dictionary::new();
                d.set("Type", Object::Name(b"Page".to_vec()));
                d.set("Parent", Object::Reference(pages_id));
                d.set("Contents", Object::Reference(content_id));
                d
            }));
            kids.push(Object::Reference(page_id));
        }
        doc.get_object_mut(pages_id)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Kids", Object::Array(kids));
        let catalog_id = doc.add_object(Object::Dictionary({
            let mut d = lopdf::Dictionary::new();
            d.set("Type", Object::Name(b"Catalog".to_vec()));
            d.set("Pages", Object::Reference(pages_id));
            d
        }));
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc.save(path).unwrap();
    }

    #[test]
    fn test_load_pdf() {
        let p = "/tmp/opencode/wm_test.pdf";
        make_pdf(4, p);
        let meta = load_pdf(p.to_string()).unwrap();
        assert_eq!(meta.page_count, 4);
        assert_eq!(meta.width, 612.0);
        assert_eq!(meta.height, 792.0);
    }

    #[test]
    fn test_load_pdf_rejects_mixed_sizes() {
        // 4-page doc where page 2 differs in size
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.add_object(Object::Dictionary({
            let mut d = lopdf::Dictionary::new();
            d.set("Type", Object::Name(b"Pages".to_vec()));
            d.set("Count", Object::Integer(2));
            d
        }));
        for (w, h) in [(612, 792), (300, 400)] {
            let content_id = doc.add_object(Object::Stream(Stream::new(
                lopdf::Dictionary::new(),
                b"1 0 0 1".to_vec(),
            )));
            let page_id = doc.add_object(Object::Dictionary({
                let mut d = lopdf::Dictionary::new();
                d.set("Type", Object::Name(b"Page".to_vec()));
                d.set("Parent", Object::Reference(pages_id));
                d.set(
                    "MediaBox",
                    Object::Array(vec![
                        Object::Integer(0),
                        Object::Integer(0),
                        Object::Integer(w),
                        Object::Integer(h),
                    ]),
                );
                d.set("Contents", Object::Reference(content_id));
                d
            }));
            let kids = {
                let pages_dict = doc.get_object_mut(pages_id).unwrap().as_dict_mut().unwrap();
                match pages_dict.get(b"Kids") {
                    Ok(Object::Array(arr)) => {
                        let mut k = arr.clone();
                        k.push(Object::Reference(page_id));
                        k
                    }
                    _ => vec![Object::Reference(page_id)],
                }
            };
            doc.get_object_mut(pages_id)
                .unwrap()
                .as_dict_mut()
                .unwrap()
                .set("Kids", Object::Array(kids));
        }
        let catalog_id = doc.add_object(Object::Dictionary({
            let mut d = lopdf::Dictionary::new();
            d.set("Type", Object::Name(b"Catalog".to_vec()));
            d.set("Pages", Object::Reference(pages_id));
            d
        }));
        doc.trailer.set("Root", Object::Reference(catalog_id));
        doc.save("/tmp/opencode/wm_mixed.pdf").unwrap();
        assert!(load_pdf("/tmp/opencode/wm_mixed.pdf".into()).is_err());
    }

    #[test]
    fn test_preview_pdf_single_page() {
        let p = "/tmp/opencode/wm_test.pdf";
        make_pdf(4, p);
        let b64 = preview_pdf(p.to_string(), 1).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
    }

    #[test]
    fn test_apply_watermark_skips_first_and_last() {
        let p = "/tmp/opencode/wm_test.pdf";
        let out = "/tmp/opencode/wm_out.pdf";
        make_pdf(4, p);
        apply_watermark(
            p.to_string(),
            out.to_string(),
            vec![TextItem {
                text: "CONFIDENTIAL".into(),
                font_size: 24.0,
                x_frac: 0.5,
                y_frac: 0.5,
                align: Some("center".into()),
            }],
            255,
            0,
            0,
            128,
        )
        .unwrap();
        let doc = Document::load(out).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 4);
        for (n, id) in &pages {
            let content = doc.get_page_content(*id);
            let content = String::from_utf8_lossy(&content);
            let has_wm = content.contains("/Im0 Do");
            assert_eq!(has_wm, n > &1 && n < &4, "page {n} watermark wrong");
        }
    }

    #[test]
    fn test_apply_watermark_multiple_texts() {
        let p = "/tmp/opencode/wm_test.pdf";
        let out = "/tmp/opencode/wm_multi.pdf";
        make_pdf(4, p);
        apply_watermark(
            p.to_string(),
            out.to_string(),
            vec![
                TextItem {
                    text: "FIRST".into(),
                    font_size: 24.0,
                    x_frac: 0.5,
                    y_frac: 0.3,
                    align: Some("left".into()),
                },
                TextItem {
                    text: "SECOND".into(),
                    font_size: 48.0,
                    x_frac: 0.25,
                    y_frac: 0.7,
                    align: Some("right".into()),
                },
            ],
            0,
            0,
            0,
            128,
        )
        .unwrap();
        let doc = Document::load(out).unwrap();
        let pages = doc.get_pages();
        for (n, id) in &pages {
            let content = String::from_utf8_lossy(&doc.get_page_content(*id)).into_owned();
            let stamped = content.contains("/Im0 Do") && content.contains("/Im1 Do");
            assert_eq!(stamped, n > &1 && n < &4, "page {n} watermarks wrong");
        }
    }

    #[test]
    fn test_render_stamp_alignment_bounds() {
        // left/center/right all produce identical ink size, just shifted
        let a = render_stamp("AB\nC", 24.0, Align::Left, 0, 0, 0, 255).unwrap();
        let b = render_stamp("AB\nC", 24.0, Align::Center, 0, 0, 0, 255).unwrap();
        let c = render_stamp("AB\nC", 24.0, Align::Right, 0, 0, 0, 255).unwrap();
        assert_eq!((a.w, a.h), (b.w, b.h));
        assert_eq!((b.w, b.h), (c.w, c.h));
        // center/right must shift glyphs rightwards within the block
        let nonzero = b
            .alpha
            .iter()
            .zip(c.alpha.iter())
            .filter(|(x, y)| x != y)
            .count();
        assert!(nonzero > 0, "alignment should change pixel layout");
    }

    #[test]
    fn debug_align_visual() {
        for (name, s) in [
            ("L", render_stamp("ABC\nD", 24.0, Align::Left, 0, 0, 0, 255).unwrap()),
            ("R", render_stamp("ABC\nD", 24.0, Align::Right, 0, 0, 0, 255).unwrap()),
        ] {
            println!("== {name} w={} h={}", s.w, s.h);
            for row in (0..s.h).step_by(8) {
                let line: String = (0..s.w)
                    .step_by(4)
                    .map(|x| if s.alpha[row * s.w + x] > 0 { '#' } else { '.' })
                    .collect();
                println!("{line}");
            }
        }
    }}
