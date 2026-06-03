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
///
/// Round V iter-2 (BkCallout decoration parity, 2026-06-03): when a
/// paragraph carries `<w:pStyle w:val="BkCallout"/>` but no flavor
/// sentinel (e.g. emitted by an uninstrumented call site such as
/// `closing_thought_block` or a future emitter), it falls back to
/// [`CalloutFlavor::Generic`] (`1F3864` border / `EEF2F8` fill). Pure
/// non-callout paragraphs are still left untouched.
///
/// Round V iter-3 (BkCallout unknown-flavor parity, 2026-06-03):
/// idempotent-normalisation extension — when a BkCallout paragraph
/// already carries pBdr+shd chrome but the (border, fill) pair does
/// NOT match any documented [`CalloutFlavor`], the chrome is stripped
/// and replaced with Generic. This closes the WARN stragglers reported
/// by `parity_icons::check_bkcallout_decoration` when an emitter
/// hard-coded an off-flavour colour pair.
fn decorate_one_paragraph(inner: &str) -> (String, bool) {
    // 1) Find a flavor sentinel `<w:bookmarkStart … w:name="BkFlavor:…"/>`.
    //    Match liberally on attribute order.
    let Some(name_start) = inner.find(r#"w:name="BkFlavor:"#) else {
        // No sentinel → handle BkCallout paragraphs as follows:
        //   (a) no chrome at all              → inject Generic chrome
        //   (b) chrome present, KNOWN flavor  → leave untouched (idempotent)
        //   (c) chrome present, UNKNOWN flavor → strip + replace with Generic
        if paragraph_is_bkcallout(inner) {
            if !paragraph_has_chrome(inner) {
                // case (a)
                let decorated = inject_chrome_into_ppr(inner, CalloutFlavor::Generic);
                return (decorated, true);
            }
            if !paragraph_chrome_is_known_flavor(inner) {
                // case (c): replace off-flavour chrome with Generic
                let decorated = inject_chrome_into_ppr(inner, CalloutFlavor::Generic);
                return (decorated, true);
            }
            // case (b) — known flavour, leave alone.
        }
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

/// Cheap sniff: does the paragraph inner carry
/// `<w:pStyle w:val="BkCallout"/>` somewhere in its pPr? Used by the
/// fallback path in [`decorate_one_paragraph`] when no flavor sentinel
/// is present. Restricts the scan to the pPr region (capped at the
/// first `<w:r` / `</w:pPr>` boundary) so a body run that happens to
/// contain the literal string `BkCallout` does not trigger a
/// false-positive decoration.
fn paragraph_is_bkcallout(inner: &str) -> bool {
    let cap = inner
        .find("</w:pPr>")
        .map(|i| i + "</w:pPr>".len())
        .or_else(|| inner.find("<w:r>"))
        .or_else(|| inner.find("<w:r "))
        .unwrap_or(inner.len());
    inner[..cap].contains(r#"w:pStyle w:val="BkCallout""#)
}

/// Cheap sniff: does the paragraph's pPr already carry BOTH `<w:pBdr>`
/// AND `<w:shd …/>` chrome? Used by the fallback path so the second
/// `apply_callout_chrome` pass on already-decorated XML is a no-op.
fn paragraph_has_chrome(inner: &str) -> bool {
    let cap = inner
        .find("</w:pPr>")
        .map(|i| i + "</w:pPr>".len())
        .unwrap_or(inner.len());
    let ppr = &inner[..cap];
    ppr.contains("<w:pBdr>") && ppr.contains("<w:shd ")
}

/// Round V iter-3 (BkCallout unknown-flavor normalization, 2026-06-03).
///
/// Read the `(border colour, shd fill)` pair from a paragraph's pPr and
/// return `true` iff it matches one of the documented [`CalloutFlavor`]
/// variants (the 5 entries `Tip / Note / Warning / Generic / Keypoints`,
/// which exactly mirror the parity gate's `REF_CALLOUT_FLAVORS`).
///
/// `false` when chrome is absent, when only one of the two is present,
/// or when the (border, fill) pair is not a known flavour. The fallback
/// path in [`decorate_one_paragraph`] uses this to overwrite off-flavour
/// chrome with `Generic`.
fn paragraph_chrome_is_known_flavor(inner: &str) -> bool {
    let cap = inner
        .find("</w:pPr>")
        .map(|i| i + "</w:pPr>".len())
        .unwrap_or(inner.len());
    let ppr = &inner[..cap];
    let Some(border) = sniff_pbdr_color(ppr) else {
        return false;
    };
    let Some(fill) = sniff_shd_fill(ppr) else {
        return false;
    };
    for flavor in [
        CalloutFlavor::Tip,
        CalloutFlavor::Note,
        CalloutFlavor::Warning,
        CalloutFlavor::Generic,
        CalloutFlavor::Keypoints,
    ] {
        if border.eq_ignore_ascii_case(flavor.border_hex())
            && fill.eq_ignore_ascii_case(flavor.fill_hex())
        {
            return true;
        }
    }
    false
}

/// Extract the FIRST `w:color="…"` attribute value inside a `<w:pBdr>`
/// block; case-preserved hex (caller compares case-insensitively).
fn sniff_pbdr_color(ppr: &str) -> Option<String> {
    let bdr_open = ppr.find("<w:pBdr>")?;
    let bdr_close = ppr[bdr_open..]
        .find("</w:pBdr>")
        .map(|n| bdr_open + n)
        .unwrap_or(ppr.len());
    let bdr = &ppr[bdr_open..bdr_close];
    let key = r#"w:color=""#;
    let k = bdr.find(key)?;
    let after = &bdr[k + key.len()..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// Extract the `w:fill="…"` attribute value inside the first
/// `<w:shd …/>` element of the pPr; case-preserved hex.
fn sniff_shd_fill(ppr: &str) -> Option<String> {
    let shd_open = ppr.find("<w:shd ")?;
    let shd_close = ppr[shd_open..]
        .find("/>")
        .map(|n| shd_open + n)
        .unwrap_or(ppr.len());
    let shd = &ppr[shd_open..shd_close];
    let key = r#"w:fill=""#;
    let k = shd.find(key)?;
    let after = &shd[k + key.len()..];
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

    /// Round V iter-2: a `BkCallout`-styled paragraph WITHOUT a flavor
    /// sentinel (e.g. emitted by `closing_thought_block`) must fall
    /// back to `Generic` (`1F3864` border / `EEF2F8` fill) so the
    /// parity sub-check sees pBdr + shd on every BkCallout paragraph.
    #[test]
    fn bkcallout_without_sentinel_falls_back_to_generic() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:r><w:t>Closing thought</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(
            did,
            "must decorate a BkCallout paragraph even without sentinel"
        );
        assert!(
            decorated.contains(r#"w:color="1F3864""#),
            "generic border colour must be 1F3864: {decorated}"
        );
        assert!(
            decorated.contains(r#"w:fill="EEF2F8""#),
            "generic fill colour must be EEF2F8: {decorated}"
        );
        let pbdr_at = decorated.find("<w:pBdr>").expect("pBdr injected");
        let shd_at = decorated.find("<w:shd ").expect("shd injected");
        assert!(pbdr_at < shd_at, "pBdr MUST precede shd in pPr");
    }

    /// Non-callout paragraphs (no `BkCallout` pStyle and no sentinel)
    /// must remain untouched — the fallback path is restricted to
    /// paragraphs that actually carry the BkCallout style.
    #[test]
    fn non_bkcallout_paragraph_unchanged_by_fallback() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkBody"/></w:pPr>"#,
            r#"<w:r><w:t>regular body text</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(!did, "non-callout paragraph must not be decorated");
        assert_eq!(decorated, para_inner);
    }

    /// Round V iter-3: a BkCallout paragraph that already carries chrome
    /// but with NON-canonical colours (e.g. a future emitter that
    /// hard-coded an off-flavour border/fill pair) must be normalised
    /// to `Generic` so the parity gate doesn't count it as
    /// `unknown_flavor`. This is the idempotent-normalization fix.
    #[test]
    fn bkcallout_with_unknown_chrome_normalized_to_generic() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/>"#,
            // Off-flavour chrome: a teal border + lavender fill that
            // matches none of the documented flavours.
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="00CED1"/></w:pBdr>"#,
            r#"<w:shd w:val="clear" w:color="auto" w:fill="DAB6FB"/>"#,
            r#"</w:pPr>"#,
            r#"<w:r><w:t>weird</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(
            did,
            "off-flavour BkCallout chrome must be normalised: {decorated}"
        );
        // The teal border must be gone, replaced by Generic navy.
        assert!(
            !decorated.contains(r#"w:color="00CED1""#),
            "off-flavour border must be stripped: {decorated}"
        );
        assert!(
            !decorated.contains(r#"w:fill="DAB6FB""#),
            "off-flavour fill must be stripped: {decorated}"
        );
        assert!(
            decorated.contains(r#"w:color="1F3864""#),
            "Generic border must replace: {decorated}"
        );
        assert!(
            decorated.contains(r#"w:fill="EEF2F8""#),
            "Generic fill must replace: {decorated}"
        );
    }

    /// A BkCallout paragraph that already carries one of the documented
    /// flavours (e.g. `Tip = 2E7D32 / EAF6EC`) must NOT be touched by
    /// the fallback path — preserves the idempotency contract.
    #[test]
    fn bkcallout_with_known_flavor_chrome_untouched() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/>"#,
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="2E7D32"/></w:pBdr>"#,
            r#"<w:shd w:val="clear" w:color="auto" w:fill="EAF6EC"/>"#,
            r#"</w:pPr>"#,
            r#"<w:r><w:t>tip body</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(!did, "known-flavour chrome must be left as-is");
        assert_eq!(decorated, para_inner);
    }

    /// The Note flavour (`1F3864 / EAF1FB`) is a documented flavour
    /// even though its fill differs from Generic. Must not be normalised.
    #[test]
    fn bkcallout_with_note_chrome_untouched() {
        let para_inner = concat!(
            r#"<w:pPr><w:pStyle w:val="BkCallout"/>"#,
            r#"<w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr>"#,
            r#"<w:shd w:val="clear" w:color="auto" w:fill="EAF1FB"/>"#,
            r#"</w:pPr>"#,
            r#"<w:r><w:t>note body</w:t></w:r>"#,
        );
        let (decorated, did) = decorate_one_paragraph(para_inner);
        assert!(!did, "Note flavour must be a recognised documented flavour");
        assert_eq!(decorated, para_inner);
    }

    /// Sniff helpers must extract the colour/fill out of plausible pPr
    /// shapes and return `None` when the attribute is missing.
    #[test]
    fn sniff_helpers_extract_colour_and_fill() {
        let ppr = r#"<w:pPr><w:pBdr><w:left w:val="single" w:sz="24" w:space="8" w:color="1F3864"/></w:pBdr><w:shd w:val="clear" w:color="auto" w:fill="EAF1FB"/></w:pPr>"#;
        assert_eq!(sniff_pbdr_color(ppr).as_deref(), Some("1F3864"));
        assert_eq!(sniff_shd_fill(ppr).as_deref(), Some("EAF1FB"));
        // Absent chrome → None.
        let bare = r#"<w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#;
        assert!(sniff_pbdr_color(bare).is_none());
        assert!(sniff_shd_fill(bare).is_none());
    }

    /// The full pipeline: a body with one sentinel-decorated callout
    /// and one bare BkCallout (no sentinel) must end up with chrome
    /// on BOTH paragraphs.
    #[test]
    fn apply_callout_chrome_decorates_bare_bkcallout_too() {
        let body = concat!(
            r#"<w:body>"#,
            // sentinelled tip
            r#"<w:p><w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:bookmarkStart w:id="100200" w:name="BkFlavor:tip"/>"#,
            r#"<w:bookmarkEnd w:id="100200"/>"#,
            r#"<w:r><w:t>tip</w:t></w:r></w:p>"#,
            // bare BkCallout (no sentinel — e.g. closing-thought)
            r#"<w:p><w:pPr><w:pStyle w:val="BkCallout"/></w:pPr>"#,
            r#"<w:r><w:t>closing</w:t></w:r></w:p>"#,
            r#"</w:body>"#,
        );
        let out = apply_callout_chrome(body);
        // Tip flavor colour appears once.
        assert_eq!(out.matches(r#"w:color="2E7D32""#).count(), 1);
        // Generic flavor colour appears once (the bare BkCallout fallback).
        assert_eq!(out.matches(r#"w:color="1F3864""#).count(), 1);
        // Both pBdr blocks present.
        assert_eq!(out.matches("<w:pBdr>").count(), 2);
        // Both shd blocks present.
        assert_eq!(out.matches("<w:shd ").count(), 2);
    }
}
