//! Inline callout chrome — per-flavor `pBdr` + `shd` injection.
//!
//! Round-V Zone-E (visual-parity, 2026-06-03). The reference AI-Norms
//! book carries a per-paragraph **left accent border** and **fill
//! shading** on every `BkCallout` paragraph: tip is green, note is
//! navy, warning is amber, generic is light-navy, keypoints is grey.
//! Splitting the style into `BkCalloutTip/Note/Warning` was rejected
//! (cross-cutting risk #4) so the per-flavor difference must live on
//! the **paragraph instance** instead.
//!
//! `docx-rs` 0.4.x exposes neither `Paragraph::shading()` nor
//! `Paragraph::borders()` publicly (the underlying `ParagraphProperty`
//! has both fields, but the setter on `Paragraph` is `pub(crate)`), so
//! we cannot emit the chrome at construction time. Instead the
//! callout helpers in `book.rs` plant a `<w:bookmarkStart>` /
//! `<w:bookmarkEnd>` sentinel pair on every `BkCallout` paragraph; the
//! sentinel name (`BkFlavor:tip` / `:note` / `:warning` / `:generic`
//! / `:keypoints`) encodes the flavor. After serialisation, the
//! [`apply_callout_chrome`] postprocess pass walks `document.xml`,
//! finds each sentinelled paragraph, removes the sentinel, and
//! injects `<w:pBdr><w:left .../></w:pBdr><w:shd .../>` into the
//! paragraph's `<w:pPr>` in correct OOXML schema order (per
//! cross-cutting risk #8 — `pBdr` MUST precede `shd`).
//!
//! Colors are literal hex values (cross-cutting risk #1 —
//! THEME-COLOR-CASCADE: no theme-accent references) so they survive
//! theme1.xml replacement by Zone B.

use std::fmt::Write as _;

/// Sentinel-name prefix planted by the callout helpers in `book.rs`.
///
/// Paragraphs that should receive per-flavor chrome carry a
/// `<w:bookmarkStart w:id="…" w:name="BkFlavor:<kind>"/>` …
/// `<w:bookmarkEnd w:id="…"/>` pair as their first two children;
/// [`apply_callout_chrome`] strips the pair after injecting chrome.
pub const FLAVOR_BOOKMARK_PREFIX: &str = "BkFlavor:";

/// The five callout flavors recognised by the chrome pass.
///
/// `Generic` covers the unflavored `callout_box`; `Keypoints` is the
/// grey "Key topics at a glance" box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutFlavor {
    Tip,
    Note,
    Warning,
    Generic,
    Keypoints,
}

impl CalloutFlavor {
    /// Sentinel name embedded in the `<w:bookmarkStart>` placeholder.
    pub const fn bookmark_name(self) -> &'static str {
        match self {
            CalloutFlavor::Tip => "BkFlavor:tip",
            CalloutFlavor::Note => "BkFlavor:note",
            CalloutFlavor::Warning => "BkFlavor:warning",
            CalloutFlavor::Generic => "BkFlavor:generic",
            CalloutFlavor::Keypoints => "BkFlavor:keypoints",
        }
    }

    /// Left-border colour (literal hex, no theme reference).
    pub const fn border_hex(self) -> &'static str {
        match self {
            CalloutFlavor::Tip => "2E7D32",
            CalloutFlavor::Note => "1F3864",
            CalloutFlavor::Warning => "C77F18",
            CalloutFlavor::Generic => "1F3864",
            CalloutFlavor::Keypoints => "8A8A8A",
        }
    }

    /// Fill (shading) colour (literal hex, no theme reference).
    pub const fn fill_hex(self) -> &'static str {
        match self {
            CalloutFlavor::Tip => "EAF6EC",
            CalloutFlavor::Note => "EAF1FB",
            CalloutFlavor::Warning => "FBF1E2",
            CalloutFlavor::Generic => "EEF2F8",
            CalloutFlavor::Keypoints => "ECECEC",
        }
    }

    fn from_name(name: &str) -> Option<CalloutFlavor> {
        match name {
            "BkFlavor:tip" => Some(CalloutFlavor::Tip),
            "BkFlavor:note" => Some(CalloutFlavor::Note),
            "BkFlavor:warning" => Some(CalloutFlavor::Warning),
            "BkFlavor:generic" => Some(CalloutFlavor::Generic),
            "BkFlavor:keypoints" => Some(CalloutFlavor::Keypoints),
            _ => None,
        }
    }
}

/// Build the raw `<w:pBdr><w:left .../></w:pBdr>` fragment for a
/// flavor.  Width matches the reference (`w:sz="24"`, eighth-points
/// → 3pt), padding `w:space="8"`, line style `single`.
pub fn pbdr_left_xml(flavor: CalloutFlavor) -> String {
    format!(
        r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="{color}"/></w:pBdr>"#,
        color = flavor.border_hex()
    )
}

/// Build the raw `<w:shd …/>` fragment for a flavor.
pub fn shd_xml(flavor: CalloutFlavor) -> String {
    format!(
        r#"<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>"#,
        fill = flavor.fill_hex()
    )
}

/// Build a horizontal-rule paragraph: an empty `<w:p>` whose only
/// chrome is a bottom border (`w:sz="12"`, navy). Used at chapter
/// boundaries and on the cover-rule replacement. Returned XML is
/// self-contained and can be `&str`-spliced into `document.xml`.
pub fn horizontal_rule_xml(color_hex: &str, sz_eighth_pt: u8) -> String {
    format!(
        r#"<w:p><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="{sz}" w:space="1" w:color="{color}"/></w:pBdr></w:pPr></w:p>"#,
        sz = sz_eighth_pt,
        color = color_hex
    )
}

/// Postprocess pass — locate every `<w:p>` that carries a
/// `<w:bookmarkStart w:name="BkFlavor:KIND"/>` sentinel, strip the
/// sentinel pair, and inject the per-flavor `<w:pBdr>` + `<w:shd>`
/// into the paragraph's `<w:pPr>` in correct OOXML schema order
/// (`pBdr` BEFORE `shd`, both AFTER `pStyle` / `keepNext` /
/// `keepLines`).
///
/// If a paragraph has no `<w:pPr>` at all, one is created. If its
/// `<w:pPr>` already contains `<w:pBdr>` or `<w:shd>` (rare —
/// shouldn't happen from the renderer today), the existing element
/// is replaced with the flavor-specific one so the chrome is
/// idempotent on a second pass.
///
/// Schema-order insertion point: per ECMA-376 the `<w:pPr>` child
/// order is `pStyle`, `keepNext`, `keepLines`, `pageBreakBefore`,
/// `frameProperty`, `widowControl`, `numPr`, `suppressLineNumbers`,
/// `pBdr`, `shd`, `tabs`, `suppressAutoHyphens`, `kinsoku`, …,
/// `spacing`, `ind`, …, `jc`, `textDirection`, `textAlignment`,
/// `outlineLvl`, `divId`, `cnfStyle`, `rPr`. We insert `pBdr` + `shd`
/// **just before** the first of `spacing`, `ind`, `jc`, `outlineLvl`,
/// `rPr`, `sectPr` if present; otherwise just before `</w:pPr>`.
pub fn apply_callout_chrome(doc: &str) -> String {
    let mut out = String::with_capacity(doc.len() + 1024);
    let mut rest = doc;
    while let Some(open_rel) = rest.find("<w:p ").or_else(|| rest.find("<w:p>")) {
        // Distinguish `<w:p ...>` from `<w:pPr>`, `<w:pBdr>`, etc.
        // `find` matched on `<w:p ` or `<w:p>` so the next char after
        // "<w:p" is either ' ' or '>' — both safe.
        out.push_str(&rest[..open_rel]);
        let after_open = open_rel
            + rest[open_rel..]
                .find('>')
                .map(|p| p + 1)
                .unwrap_or(rest[open_rel..].len());
        // Locate matching </w:p>. Paragraphs are non-nesting in OOXML.
        let Some(close_rel) = rest[after_open..].find("</w:p>") else {
            out.push_str(&rest[open_rel..]);
            return out;
        };
        let close_abs = after_open + close_rel;
        let para_open = &rest[open_rel..after_open]; // "<w:p ...>"
        let para_inner = &rest[after_open..close_abs];
        let para_close = "</w:p>";

        let (rewritten_inner, decorated) = decorate_one_paragraph(para_inner);
        out.push_str(para_open);
        out.push_str(&rewritten_inner);
        out.push_str(para_close);

        // Cosmetic guard: ensure we made forward progress even if the
        // decorate step was a no-op (it always returns the original
        // inner unchanged when no sentinel was found).
        let _ = decorated;

        rest = &rest[close_abs + para_close.len()..];
    }
    out.push_str(rest);
    out
}

/// Returns `(new_inner, true)` if a flavor sentinel was found and
/// decorated, or `(old_inner.to_string(), false)` otherwise. Public
/// for test access only.
fn decorate_one_paragraph(inner: &str) -> (String, bool) {
    // 1) Find a flavor sentinel `<w:bookmarkStart … w:name="BkFlavor:…"/>`.
    //    Match liberally on attribute order.
    let Some(name_start) = inner.find(r#"w:name="BkFlavor:"#) else {
        return (inner.to_string(), false);
    };
    // Recover the flavor name (closing quote terminates it).
    let after = &inner[name_start + r#"w:name=""#.len()..];
    let Some(end_quote) = after.find('"') else {
        return (inner.to_string(), false);
    };
    let flavor_name = &after[..end_quote];
    let Some(flavor) = CalloutFlavor::from_name(flavor_name) else {
        return (inner.to_string(), false);
    };
    // 2) Locate the whole `<w:bookmarkStart .../>` self-closing tag
    //    (we know it terminates with `/>` since docx-rs emits it
    //    self-closing).
    let start_tag_open = inner[..name_start]
        .rfind("<w:bookmarkStart")
        .expect("name attr lives on a bookmarkStart tag");
    let after_start_close = name_start + r#"w:name=""#.len() + end_quote + 1;
    let Some(start_tag_close_rel) = inner[after_start_close..].find("/>") else {
        return (inner.to_string(), false);
    };
    let start_tag_end = after_start_close + start_tag_close_rel + "/>".len();
    // 3) Recover the bookmark id so we can match the closing tag.
    let start_tag = &inner[start_tag_open..start_tag_end];
    let id = extract_bookmark_id(start_tag).unwrap_or_default();
    // 4) Locate `<w:bookmarkEnd w:id="<id>"/>` after the start tag.
    let end_marker = format!(r#"<w:bookmarkEnd w:id="{id}"/>"#);
    let (end_marker_pos, end_marker_len) = if let Some(p) = inner[start_tag_end..].find(&end_marker)
    {
        (start_tag_end + p, end_marker.len())
    } else {
        // Fallback: docx-rs may emit a space before "/>": tolerate.
        let alt = format!(r#"<w:bookmarkEnd w:id="{id}" />"#);
        if let Some(p) = inner[start_tag_end..].find(&alt) {
            (start_tag_end + p, alt.len())
        } else {
            // Leave the bookmarkEnd in place if we can't find it; only
            // strip the start tag. The chrome still gets injected.
            (start_tag_end, 0)
        }
    };

    // 5) Excise the sentinel tags from the paragraph body.
    let mut stripped = String::with_capacity(inner.len());
    stripped.push_str(&inner[..start_tag_open]);
    if end_marker_len == 0 {
        stripped.push_str(&inner[start_tag_end..]);
    } else {
        stripped.push_str(&inner[start_tag_end..end_marker_pos]);
        stripped.push_str(&inner[end_marker_pos + end_marker_len..]);
    }

    // 6) Inject pBdr + shd into the pPr (creating pPr if absent),
    //    preserving OOXML schema order: pBdr before shd, both after
    //    pStyle/keepNext/keepLines but before spacing/ind/jc/rPr.
    let decorated = inject_chrome_into_ppr(&stripped, flavor);
    (decorated, true)
}

fn extract_bookmark_id(start_tag: &str) -> Option<String> {
    let key = "w:id=\"";
    let idx = start_tag.find(key)?;
    let after = &start_tag[idx + key.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// Splice `<w:pBdr>` + `<w:shd>` into a paragraph body's pPr (creating
/// pPr if absent). Existing pBdr / shd elements are replaced so the
/// pass is idempotent.
fn inject_chrome_into_ppr(para_inner: &str, flavor: CalloutFlavor) -> String {
    let pbdr = pbdr_left_xml(flavor);
    let shd = shd_xml(flavor);

    // Case A: paragraph has an existing <w:pPr>…</w:pPr>.
    if let Some(ppr_open) = para_inner.find("<w:pPr>")
        && let Some(ppr_close_rel) = para_inner[ppr_open..].find("</w:pPr>")
    {
        let ppr_close = ppr_open + ppr_close_rel;
        let pre = &para_inner[..ppr_open + "<w:pPr>".len()];
        let body = &para_inner[ppr_open + "<w:pPr>".len()..ppr_close];
        let post = &para_inner[ppr_close..];

        // Strip any existing pBdr / shd so we are idempotent.
        let body_no_old = strip_existing_pbdr_shd(body);

        // Find first occurrence of an element that schema-orders AFTER
        // pBdr/shd. We insert just before it. Order: pBdr → shd →
        // tabs/spacing/ind/jc/sectPr/rPr. Note: `<w:rPr>` is emitted
        // by docx-rs as the FIRST child of pPr (in violation of strict
        // schema order, but Word tolerates it); we treat rPr as a
        // "before" element for splice purposes — we insert AFTER rPr.
        let split = pick_split_after_chrome(&body_no_old);
        let mut out = String::with_capacity(para_inner.len() + pbdr.len() + shd.len() + 16);
        out.push_str(pre);
        out.push_str(&body_no_old[..split]);
        out.push_str(&pbdr);
        out.push_str(&shd);
        out.push_str(&body_no_old[split..]);
        out.push_str(post);
        return out;
    }

    // Case B: pPr absent. Insert one at the very start of the
    // paragraph (before any run/bookmark/etc.).
    let mut out = String::with_capacity(para_inner.len() + pbdr.len() + shd.len() + 16);
    out.push_str("<w:pPr>");
    out.push_str(&pbdr);
    out.push_str(&shd);
    out.push_str("</w:pPr>");
    out.push_str(para_inner);
    out
}

/// Inside an existing `<w:pPr>` body (text between `<w:pPr>` and
/// `</w:pPr>`), pick the byte offset at which `<w:pBdr>` + `<w:shd>`
/// should be inserted to preserve OOXML schema order.
///
/// Splice rule: insert **before** the first of (in this order)
/// `<w:tabs>`, `<w:spacing>`, `<w:ind>`, `<w:jc>`, `<w:textAlignment>`,
/// `<w:outlineLvl>`, `<w:sectPr>`. If none are present, splice at the
/// end of the body (just before `</w:pPr>`).
///
/// Elements that come BEFORE chrome in schema order
/// (`pStyle`/`keepNext`/`keepLines`/`numPr`/`rPr` as emitted by
/// docx-rs) are skipped — their positions stay untouched.
fn pick_split_after_chrome(body: &str) -> usize {
    let after_markers = [
        "<w:tabs>",
        "<w:tabs ",
        "<w:spacing ",
        "<w:spacing/>",
        "<w:ind ",
        "<w:ind/>",
        "<w:jc ",
        "<w:textAlignment ",
        "<w:outlineLvl ",
        "<w:sectPr>",
        "<w:sectPr ",
    ];
    let mut best: Option<usize> = None;
    for m in after_markers {
        if let Some(p) = body.find(m) {
            best = Some(best.map_or(p, |b| b.min(p)));
        }
    }
    best.unwrap_or(body.len())
}

/// Strip a leading-or-mid `<w:pBdr>…</w:pBdr>` and `<w:shd …/>` from
/// a pPr body so [`inject_chrome_into_ppr`] is idempotent on
/// re-application. Both are self-or-paired-closing in well-formed
/// OOXML; we handle both shapes defensively.
fn strip_existing_pbdr_shd(body: &str) -> String {
    let mut s = body.to_string();
    // pBdr is always paired (it has child <w:left>/<w:top>/etc).
    while let Some(open) = s.find("<w:pBdr>") {
        if let Some(close_rel) = s[open..].find("</w:pBdr>") {
            let close = open + close_rel + "</w:pBdr>".len();
            s.replace_range(open..close, "");
        } else {
            break;
        }
    }
    // shd is always self-closing in docx-rs output.
    while let Some(open) = s.find("<w:shd ") {
        if let Some(close_rel) = s[open..].find("/>") {
            let close = open + close_rel + "/>".len();
            s.replace_range(open..close, "");
        } else {
            break;
        }
    }
    s
}

/// Build a `(name, id)` argument pair to thread through
/// `Paragraph::add_bookmark_start(id, name).add_bookmark_end(id)` in
/// the callout helpers. Bookmark ids are pulled from a per-document
/// counter held by the caller (see [`FlavorBookmarkCounter`]).
pub fn flavor_bookmark_name(flavor: CalloutFlavor) -> String {
    flavor.bookmark_name().to_string()
}

/// Monotonic counter for flavor-bookmark ids. The counter is private
/// to the renderer; callers thread a `&mut FlavorBookmarkCounter`
/// through `admonition_box` / `callout_box` / `keypoints_box`. Ids
/// must be unique within a document so we offset by a large
/// constant (`100_000`) to avoid collisions with docx-rs's own
/// bookmarks (TOC, etc., which start at 0).
#[derive(Debug, Default)]
pub struct FlavorBookmarkCounter {
    next: usize,
}

impl FlavorBookmarkCounter {
    pub fn new() -> Self {
        Self { next: 0 }
    }
    pub fn issue(&mut self) -> usize {
        let id = 100_000 + self.next;
        self.next += 1;
        id
    }
}

/// Helper used by tests + the apply pass to fold the trace metadata
/// into a single human-readable line. Kept here so the test crate
/// doesn't have to reach into private helpers.
pub fn debug_dump(flavor: CalloutFlavor) -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "{} border={} fill={}",
        flavor.bookmark_name(),
        flavor.border_hex(),
        flavor.fill_hex()
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbdr_xml_per_flavor() {
        assert_eq!(
            pbdr_left_xml(CalloutFlavor::Tip),
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="2E7D32"/></w:pBdr>"#
        );
        assert_eq!(
            pbdr_left_xml(CalloutFlavor::Warning),
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="C77F18"/></w:pBdr>"#
        );
        assert_eq!(
            pbdr_left_xml(CalloutFlavor::Keypoints),
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="8A8A8A"/></w:pBdr>"#
        );
    }

    #[test]
    fn shd_xml_per_flavor() {
        assert_eq!(
            shd_xml(CalloutFlavor::Tip),
            r#"<w:shd w:val="clear" w:color="auto" w:fill="EAF6EC"/>"#
        );
        assert_eq!(
            shd_xml(CalloutFlavor::Note),
            r#"<w:shd w:val="clear" w:color="auto" w:fill="EAF1FB"/>"#
        );
    }

    /// pBdr must precede shd inside <w:pPr> (cross-cutting risk #8 —
    /// OOXML schema is order-strict).
    #[test]
    fn injection_preserves_pbdr_before_shd() {
        // Minimal paragraph with a flavor sentinel.
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/><w:keepLines/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100000" w:name="BkFlavor:tip"/>"#,
            r#"<w:bookmarkEnd w:id="100000"/>"#,
            r#"<w:r><w:t>body</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(did, "sentinel must be matched");
        let pbdr_at = decorated.find("<w:pBdr>").expect("pBdr injected");
        let shd_at = decorated.find("<w:shd ").expect("shd injected");
        assert!(
            pbdr_at < shd_at,
            "pBdr MUST appear before shd in pPr (OOXML schema order); got pbdr@{pbdr_at} shd@{shd_at}: {decorated}"
        );
        // Both inside the original pPr block.
        let ppr_close = decorated.find("</w:pPr>").expect("pPr closes");
        assert!(shd_at < ppr_close, "shd must live inside pPr");
        // Sentinel was stripped.
        assert!(!decorated.contains("BkFlavor:tip"));
        assert!(!decorated.contains("w:bookmarkStart"));
        assert!(!decorated.contains("w:bookmarkEnd"));
    }

    #[test]
    fn injection_picks_correct_flavor_colors() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100001" w:name="BkFlavor:warning"/>"#,
            r#"<w:bookmarkEnd w:id="100001"/>"#,
            r#"<w:r><w:t>danger</w:t></w:r>"#,
        );
        let (decorated, _) = decorate_one_paragraph(para_inner);
        assert!(decorated.contains(r#"w:color="C77F18""#));
        assert!(decorated.contains(r#"w:fill="FBF1E2""#));
    }

    #[test]
    fn injection_is_idempotent() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100002" w:name="BkFlavor:note"/>"#,
            r#"<w:bookmarkEnd w:id="100002"/>"#,
            r#"<w:r><w:t>n</w:t></w:r>"#,
        );
        let (once, _) = decorate_one_paragraph(para_inner);
        // Re-run on the already-decorated paragraph (no sentinel) — must
        // be a no-op.
        let (twice, did) = decorate_one_paragraph(&once);
        assert!(!did, "no sentinel left, must be no-op");
        assert_eq!(once, twice);
    }

    #[test]
    fn apply_callout_chrome_handles_multiple_paragraphs() {
        let body = concat!(
            r#"<w:body>"#,
            // plain paragraph — untouched
            r#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#,
            // tip callout
            r#"<w:p><w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100100" w:name="BkFlavor:tip"/>"#,
            r#"<w:bookmarkEnd w:id="100100"/>"#,
            r#"<w:r><w:t>tip-body</w:t></w:r></w:p>"#,
            // note callout
            r#"<w:p><w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100101" w:name="BkFlavor:note"/>"#,
            r#"<w:bookmarkEnd w:id="100101"/>"#,
            r#"<w:r><w:t>note-body</w:t></w:r></w:p>"#,
            r#"</w:body>"#,
        );
        let out = apply_callout_chrome(body);
        // Each flavor color must appear once.
        assert_eq!(out.matches(r#"w:fill="EAF6EC""#).count(), 1);
        assert_eq!(out.matches(r#"w:fill="EAF1FB""#).count(), 1);
        assert_eq!(out.matches(r#"w:color="2E7D32""#).count(), 1);
        assert_eq!(out.matches(r#"w:color="1F3864""#).count(), 1);
        // Plain paragraph unchanged.
        assert!(out.contains("<w:p><w:r><w:t>plain</w:t></w:r></w:p>"));
        // All sentinels stripped.
        assert!(!out.contains("BkFlavor:"));
    }

    #[test]
    fn flavor_counter_yields_unique_ids() {
        let mut c = FlavorBookmarkCounter::new();
        let a = c.issue();
        let b = c.issue();
        assert_ne!(a, b);
        assert!(a >= 100_000 && b >= 100_000);
    }

    #[test]
    fn horizontal_rule_xml_shape() {
        let s = horizontal_rule_xml("1F3864", 12);
        assert!(s.contains(r#"<w:bottom w:val="single" w:sz="12" w:space="1" w:color="1F3864"/>"#));
        assert!(s.starts_with("<w:p>") && s.ends_with("</w:p>"));
    }
}
