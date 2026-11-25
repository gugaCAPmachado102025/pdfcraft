//! The Crop tool (Acrobat: Edit ▸ Crop pages): drag a rectangle on a page to crop it to that
//! rectangle; double-click a page to open Set Page Boxes for it.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{BoxSpec, DocId, Edit, PageBox, PageOrientation, PageParity, filter_pages};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;

/// A crop rectangle being dragged: (page, start on screen).
pub type CropDrag = Option<(usize, Pos2)>;

/// A crop rectangle drawn: its page and the rectangle in page view points [x0, y0, x1, y1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropDone {
    pub page: usize,
    pub view_rect: [f32; 4],
}

/// The pages a drawn crop applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CropPages {
    /// The page it was drawn on.
    #[default]
    Current,
    All,
    Even,
    Odd,
}

/// Crop pages dialog state: the crop drawn and the document it was drawn on, as it was then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropDraft {
    pub doc: DocId,
    pub generation: u64,
    pub done: CropDone,
    pub pages: CropPages,
}

/// The edit that crops `which` pages to the rectangle drawn: the same place on each page as it is displayed, cut to
/// the page. A page the rectangle doesn't reach is left alone; `None` when no page is cropped.
pub fn crop_edit(info: &DocInfo, done: &CropDone, which: CropPages) -> Option<Edit> {
    let parity = match which {
        CropPages::Current => None,
        CropPages::All => Some(PageParity::Both),
        CropPages::Even => Some(PageParity::Even),
        CropPages::Odd => Some(PageParity::Odd),
    };
    let pages = match parity {
        None => vec![done.page],
        Some(parity) => filter_pages(info, &(0..info.pages.len()).collect::<Vec<_>>(), parity, PageOrientation::Both),
    };
    let mut groups: Vec<([f64; 4], Vec<usize>)> = Vec::new();
    for p in pages {
        let Some(page) = info.pages.get(p) else { continue };
        let [x0, y0, x1, y1] = done.view_rect;
        let (x0, y0, x1, y1) = (x0.max(0.0), y0.max(0.0), x1.min(page.width), y1.min(page.height));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let (a, b) = (page.view_to_user(x0, y0).map(f64::from), page.view_to_user(x1, y1).map(f64::from));
        let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        match groups.iter_mut().find(|(r, _)| *r == rect) {
            Some((_, v)) => v.push(p),
            None => groups.push((rect, vec![p])),
        }
    }
    let mut edits: Vec<Edit> =
        groups.into_iter().map(|(rect, pages)| Edit::SetPageBox { pages, which: PageBox::Crop, spec: BoxSpec::Rect(rect) }).collect();
    if edits.len() > 1 {
        return Some(Edit::Batch { label: "Crop pages".into(), edits });
    }
    edits.pop()
}

/// A rectangle was drawn: a one-page document is cropped to it, any other is asked which pages to crop.
pub(crate) fn finish(app: &mut PdfCraftApp, index: usize, done: CropDone) {
    let doc = app.views[index].id;
    let Some(d) = app.session.get(doc) else { return };
    if d.info.pages.len() < 2 {
        app.views[index].pending_edit = crop_edit(&d.info, &done, CropPages::Current);
        return;
    }
    app.crop_draft = Some(CropDraft { doc, generation: d.edit_generation(), done, pages: CropPages::Current });
    app.dialog = Some(crate::Dialog::CropPages);
}

/// Draw the Crop pages dialog's body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, _t: &Tokens) -> (bool, bool) {
    let Some(d) = app.crop_draft.as_mut() else { return (false, true) };
    ui.label(egui::RichText::new(tl!("Crop pages")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let current = crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(d.done.page + 1).to_string())]);
    widgets::radio_value(ui, &mut d.pages, CropPages::Current, current);
    widgets::radio_value(ui, &mut d.pages, CropPages::All, tl!("All pages"));
    widgets::radio_value(ui, &mut d.pages, CropPages::Even, tl!("Even pages"));
    widgets::radio_value(ui, &mut d.pages, CropPages::Odd, tl!("Odd pages"));
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        apply = widgets::pill_button(ui, tl!("OK"), true).clicked();
        cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    (apply, cancel)
}

/// The Crop pages dialog was confirmed. Another document became active, or the document changed, while it was open:
/// the crop may not be where it was drawn, so nothing is cropped.
pub(crate) fn apply(app: &mut PdfCraftApp) {
    let Some(d) = app.crop_draft.take() else { return };
    if app.active_ids().is_none_or(|(_, id)| id != d.doc) {
        return;
    }
    let Some(doc) = app.session.get(d.doc).filter(|doc| doc.edit_generation() == d.generation) else { return };
    match crop_edit(&doc.info, &d.done, d.pages) {
        Some(edit) => {
            app.apply_edit(edit);
        }
        None => app.notify_tr("No pages match those choices"),
    }
}

/// Crop gestures on one page. Returns `Some(true)` to open Set Page Boxes for this page.
pub(crate) struct CropPageInput<'a> {
    pub(crate) resp: &'a egui::Response,
    pub(crate) xf: &'a PageXform,
    pub(crate) ptr: &'a PagePointer,
    pub(crate) page: usize,
    pub(crate) view: &'a mut DocView,
    pub(crate) allowed: bool,
    /// Nobody holds the press, and it began on this page: the tool's.
    pub(crate) free: bool,
}

pub(crate) fn page_input(ui: &egui::Ui, input: CropPageInput<'_>) -> bool {
    let CropPageInput { resp, xf, ptr, page, view, allowed, free } = input;
    let (pointer, origin) = (ptr.pos, ptr.origin);
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        view.cursor.claim(Layer::ToolArea, if allowed { egui::CursorIcon::Crosshair } else { egui::CursorIcon::NotAllowed });
    }
    if !allowed {
        return false;
    }
    // The tool's own press, of the primary button: one nobody holds.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && free
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.crop_drag = Some((page, o));
    }
    if let Some((p, start)) = view.crop_drag
        && p == page
    {
        // Where the pointer is, or where it went down when it is off the window.
        let end = pointer.unwrap_or(start);
        let end = Pos2::new(end.x.clamp(xf.rect.left(), xf.rect.right()), end.y.clamp(xf.rect.top(), xf.rect.bottom()));
        let r = Rect::from_two_pos(start, end);
        // Dim what will be cut away.
        let painter = ui.painter();
        for band in [
            Rect::from_min_max(xf.rect.min, Pos2::new(xf.rect.right(), r.top())),
            Rect::from_min_max(Pos2::new(xf.rect.left(), r.bottom()), xf.rect.max),
            Rect::from_min_max(Pos2::new(xf.rect.left(), r.top()), Pos2::new(r.left(), r.bottom())),
            Rect::from_min_max(Pos2::new(r.right(), r.top()), Pos2::new(xf.rect.right(), r.bottom())),
        ] {
            painter.rect_filled(band, CornerRadius::ZERO, Color32::from_black_alpha(70));
        }
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        if resp.drag_stopped() || !ptr.primary_down {
            view.crop_drag = None;
            if r.width() >= 4.0 && r.height() >= 4.0 {
                // In view space, so a rotated view's corners are put back in order.
                let (a, b) = (xf.screen_to_view(r.left_top()), xf.screen_to_view(r.right_bottom()));
                view.crop_done = Some(CropDone { page, view_rect: [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)] });
            }
        }
    }
    resp.double_clicked() && pointer.is_some_and(|p| xf.rect.contains(p))
}
