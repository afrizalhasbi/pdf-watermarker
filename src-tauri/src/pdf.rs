use base64::Engine;
use lopdf::{Document, Object, ObjectId};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct Meta {
    width: f64,
    height: f64,
    page_count: u32,
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
        id = dict.get(b"Parent").and_then(Object::as_reference).map_err(err)?;
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

/// Helvetica AFM advance widths (units/1000 em) for chars 32..=126.
/// Arial (the Windows substitute) is metrically identical.
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn text_width(text: &str, font_size: f64) -> f64 {
    let units: f64 = text
        .chars()
        .map(|c| {
            let u = c as u32;
            if (32..=126).contains(&u) {
                HELVETICA_WIDTHS[(u - 32) as usize] as f64
            } else {
                556.0
            }
        })
        .sum();
    units * font_size / 1000.0
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn apply_watermark(
    path: String,
    out_path: String,
    text: String,
    x_frac: f64,
    y_frac: f64,
    font_size: f64,
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
    if !(0.0..=1.0).contains(&x_frac) || !(0.0..=1.0).contains(&y_frac) {
        return Err("watermark position out of bounds".into());
    }

    let font_id = doc.add_object(Object::Dictionary({
        let mut d = lopdf::Dictionary::new();
        d.set("Type", Object::Name(b"Font".to_vec()));
        d.set("Subtype", Object::Name(b"Type1".to_vec()));
        d.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        d.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        d.set("FirstChar", Object::Integer(32));
        d.set("LastChar", Object::Integer(126));
        d.set(
            "Widths",
            Object::Array(HELVETICA_WIDTHS.iter().map(|&w| Object::Integer(w as i64)).collect()),
        );
        d
    }));

    let alpha = a as f32 / 255.0;
    let gs_id = doc.add_object(Object::Dictionary({
        let mut d = lopdf::Dictionary::new();
        d.set("Type", Object::Name(b"ExtGState".to_vec()));
        d.set("ca", Object::Real(alpha));
        d.set("CA", Object::Real(alpha));
        d
    }));

    let escaped = text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
    let tw = text_width(&text, font_size);
    let color = format!(
        "{} {} {}",
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0
    );

    // watermark every page except first and last
    for (_, page_id) in pages.iter().skip(1).take(pages.len().saturating_sub(2)) {
        let mb = mediabox(&doc, *page_id)?;
        let (w, h) = ((mb[2] - mb[0]).abs(), (mb[3] - mb[1]).abs());
        // drag position = visual center of the text:
        // x centered on width, baseline 0.36em below center (half cap-height)
        let x = mb[0] + x_frac * w - tw / 2.0;
        let y = mb[1] + (1.0 - y_frac) * h - 0.36 * font_size;
        println!(
            "[wm] page: MediaBox=({},{},{},{}) w={w} h={h} x_frac={x_frac} y_frac={y_frac} -> x={x} y={y} tw={tw}",
            mb[0], mb[1], mb[2], mb[3]
        );

        // ensure page resources have our font + ExtGState
        let resources = ensure_resources(&mut doc, *page_id)?;
        let (font_val, gs_val) = {
            let res = doc.get_object(resources).map_err(err)?;
            let d = Object::as_dict(res).map_err(|_| "resources not a dictionary")?;
            (d.get(b"Font").cloned().ok(), d.get(b"ExtGState").cloned().ok())
        };
        let font_entry = match font_val {
            Some(Object::Reference(id)) => id,
            Some(o @ Object::Dictionary(_)) => doc.add_object(o),
            _ => doc.add_object(Object::Dictionary(lopdf::Dictionary::new())),
        };
        let gs_entry = match gs_val {
            Some(Object::Reference(id)) => id,
            Some(o @ Object::Dictionary(_)) => doc.add_object(o),
            _ => doc.add_object(Object::Dictionary(lopdf::Dictionary::new())),
        };
        let font_obj = doc
            .get_object_mut(font_entry)
            .and_then(Object::as_dict_mut)
            .map_err(err)?;
        font_obj.set("F1", Object::Reference(font_id));
        let gs_obj = doc
            .get_object_mut(gs_entry)
            .and_then(Object::as_dict_mut)
            .map_err(err)?;
        gs_obj.set("GS0", Object::Reference(gs_id));

        let ops = format!(
            "\nq /GS0 gs BT /F1 {} Tf {} rg 1 0 0 1 {} {} Tm ({}) Tj ET Q\n",
            font_size, color, x, y, escaped
        );
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

    fn make_pdf(n_pages: u32, path: &str) {
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
                d.set(
                    "Contents",
                    Object::Reference(content_id),
                );
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
            let content_id = doc
                .add_object(Object::Stream(Stream::new(lopdf::Dictionary::new(), b"1 0 0 1".to_vec())));
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
            "CONFIDENTIAL".into(),
            0.5,
            0.5,
            24.0,
            255,
            0,
            0,
            128,
        )
        .unwrap();
        let doc = Document::load(out).unwrap();
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 4);
        // expected centered position for "CONFIDENTIAL" @24pt on 612x792
        let expected_x = 0.5 * 612.0 - text_width("CONFIDENTIAL", 24.0) / 2.0;
        let expected_y = 0.5 * 792.0 - 0.36 * 24.0;
        let expected_tm = format!("1 0 0 1 {} {}", expected_x, expected_y);
        for (n, id) in &pages {
            let content = doc.get_page_content(*id);
            let content = String::from_utf8_lossy(&content);
            let has_wm = content.contains("CONFIDENTIAL");
            assert_eq!(has_wm, n > &1 && n < &4, "page {n} watermark wrong");
            if has_wm {
                assert!(
                    content.contains(&expected_tm),
                    "page {n} not centered: expected {expected_tm} in {content}"
                );
            }
        }
    }
}
