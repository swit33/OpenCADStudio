//! In-place table cell editor: the cell's own text, edited where it is drawn.
//!
//! The table's picture block holds the drawn cell text, so editing a cell means
//! (a) keeping that block and (b) hiding the one text being edited while the
//! editor draws it live. Everything else about the table stays exactly as the
//! file drew it — see `entities::table::patch_block_cell_text`.
//!
//! Keystrokes come from an invisible focused `text_editor` pinned over the cell;
//! the visible text, caret and selection are the shared `view::text_surface`
//! canvas, laid out with the cell's own text style, height, rotation and wrap
//! width, at the view's own pixels-per-world scale. So what the editor shows is
//! what the table draws on commit.
//!
//! # Lifetime rules
//!
//! The editor is host state, not a command, and it hides the cell's drawn text by
//! BLANKING it in the picture block. That mutation is what makes the rules below
//! necessary — they are collected here so no future path forgets one:
//!
//! 1. **One editor, one cell.** Opening another cell cancels the open one first
//!    (`open_table_cell_edit`), or the previous cell stays blank in the drawing.
//! 2. **Every close restores.** `table_cell_commit` and `table_cell_cancel` are the
//!    only two exits, and both put the drawn text back. Anything that used to drop
//!    the state directly (`table_cell_edit = None`) is a bug.
//! 3. **A command ending closes it.** `command_driver` cancels on the sidebar's
//!    end-of-command hook, so a ribbon command never leaves a blank cell behind.
//! 4. **Anything that writes the document settles it first.**
//!    `settle_table_cell_edit` commits before a save or an autosave (SAVETIME):
//!    the file — and the `.sv$` recovery copy — must never receive the blank
//!    the editor is holding.
//! 5. **The keyboard has one owner, chosen by the last thing the user touched.**
//!    Opening the cell (or clicking its text) claims it; typing into the command
//!    line releases it (`cell_keyboard`). `focus_cmd_input` only redirects to the
//!    sink while the cell owns it, so the capture is a focus choice rather than a
//!    lock: the command line stays reachable while an editor is open.
//! 6. **Clicks never unfocus.** The surface reports clicks as `TableCellSelStart` /
//!    `TableCellSelTo` (not the MText ones, whose handler unfocuses widgets), so
//!    clicking inside the text moves the sink's caret and keeps typing in the cell.
//! 7. **Undo belongs to the cell.** Ctrl+Z with the editor open goes to the sink
//!    only (`Message::Undo` returns early), so one press undoes one character, not
//!    a character plus a drawing step.
use codec::{EntityType, Handle, MText};
use iced::widget::text_editor;

use crate::modules::annotate::table_cmd::TableCellEditStart;

/// Widget id of the keystroke sink, focused when the editor opens.
pub(in crate::app) const TABLE_CELL_SINK_ID: &str = "table_cell_edit_sink";

/// Focus the open cell editor's keystroke sink, so typing goes into the cell.
pub(in crate::app) fn focus_sink() -> iced::Task<crate::app::Message> {
    iced::widget::operation::focus(iced::widget::Id::new(TABLE_CELL_SINK_ID))
}

/// Open in-place table cell editor.
pub(crate) struct TableCellEditState {
    pub handle: Handle,
    pub row: usize,
    pub column: usize,
    /// The tab the edited cell belongs to. The handle is document-local and the
    /// editor keeps its cell blanked in THAT document, so it has to know which
    /// one to put back — and must never keep running after the user leaves it.
    pub tab_id: u64,
    /// The text being edited, as the sink holds it: VISIBLE text only (formatting
    /// codes are carried by `cells`), with `\n` for a paragraph break.
    pub content: text_editor::Content,
    /// The cell's visible text as formatting-carrying cells: the same flat index
    /// space the caret and the glyph boxes live in, and the thing the commit
    /// serializes back into an MText value. Editing through the sink keeps every
    /// code the user did not touch — a bold run in a cell stays bold.
    pub cells: Vec<super::mtext_editor::Cell>,
    /// The first paragraph's properties, the default for inserted paragraphs.
    pub para0: codec::entities::mtext_format::ParagraphProperties,
    /// The value the cell had when the editor opened (serialized from `cells`),
    /// so an unchanged edit writes nothing.
    pub original_value: String,
    /// Caret as a visible-character offset into the laid-out text.
    pub caret: usize,
    pub caret_blink_on: bool,
    /// What the cell held when the editor opened: an unchanged commit writes
    /// nothing, and Esc has nothing to restore (the drawing was never touched).
    pub original: String,
    /// Whether the editor blanked the table's own text for this cell: the cell
    /// then shows only the live surface, with nothing painted over it.
    pub blanked: bool,
    /// Live layout of the value with the cell's own style, and the frame that
    /// maps it onto the cell's screen rectangle.
    pub wires: Vec<crate::scene::WireModel>,
    pub boxes: Vec<crate::entities::text_support::GlyphBox>,
    /// The cell's corners in the cell-local frame, [x0, y0, x1, y1] with y
    /// running UP: y0 is the bottom edge (-row height), y1 the top (0).
    pub cell_world: [f32; 4],
    /// The cell frame joined with whatever the live run covers, in the same
    /// [x0, y0, x1, y1] form. Rotated cell text runs past the cell's own
    /// rectangle (a vertical run reads upward past the top edge), and a canvas
    /// clips at its bounds — without the union, the end of a vertical run and
    /// the caret there would be cut away.
    pub extent: [f32; 4],
    /// The ink the live text is drawn with: the caret uses it too, so it stays
    /// visible on the sheet the cell sits on.
    pub ink: iced::Color,
}

/// Where the editor's canvas goes on screen for a given cell rectangle: the
/// drawing's scale, the world frame the surface maps from, and the pixels it
/// occupies. Recomputed every frame — the drawing pans and zooms under the
/// editor, and the overlay has to follow it.
pub(crate) struct CellScreen {
    pub scale: f32,
    pub minx: f32,
    pub miny: f32,
    pub paint: iced::Rectangle,
}

impl TableCellEditState {
    /// Map the cell's world frame onto the cell's current screen rectangle.
    /// Both frames are [x0, y0, x1, y1] with y running UP: y0 is the bottom edge
    /// and y1 the top (the cell's own top edge is 0, its bottom -height).
    pub(crate) fn screen(&self, rect: iced::Rectangle) -> CellScreen {
        let cell_width = (self.cell_world[2] - self.cell_world[0]).max(1.0e-9);
        let scale = rect.width / cell_width;
        let [left, bottom, right, top] = self.extent;
        let paint = iced::Rectangle {
            x: rect.x + (left - self.cell_world[0]) * scale,
            // The cell's top edge sits on the rectangle's top edge; a run that
            // reaches above it (rotated text) pushes the canvas up.
            y: rect.y + (self.cell_world[3] - top) * scale,
            width: (right - left) * scale,
            height: (top - bottom) * scale,
        };
        CellScreen {
            scale,
            minx: left,
            miny: bottom,
            paint,
        }
    }
}

impl super::OpenCADStudio {
    /// Open the in-place editor on cell (`row`, `column`) and hide the text the
    /// picture block draws there, so the live surface is the only text on the
    /// cell.
    pub(crate) fn open_table_cell_edit(
        &mut self,
        handle: Handle,
        row: usize,
        column: usize,
        rect: iced::Rectangle,
    ) -> TableCellEditStart {
        let i = self.active_tab;
        // One cell editor at a time: opening another first closes the previous
        // one properly. Its text was blanked in the picture block, and dropping
        // the state without restoring it left that cell empty in the drawing.
        if self.table_cell_edit.is_some() {
            self.table_cell_cancel();
        }
        let Some(EntityType::Table(table)) = self.tabs[i].scene.document.get_entity(handle) else {
            return TableCellEditStart::NoCell;
        };
        // A click may land anywhere inside a merged span: its text lives in
        // the owner cell, so the editor (and the commit) work on that.
        let (row, column) = crate::entities::table::merged_owner_cell(table, row, column);
        let row_data = table.rows.get(row);
        if row_data
            .and_then(|row| row.cells.get(column))
            .is_none_or(|cell| {
                cell.state.intersects(
                    codec::entities::table::CellStateFlags::CONTENT_LOCKED
                        | codec::entities::table::CellStateFlags::CONTENT_READ_ONLY,
                )
            })
        {
            return TableCellEditStart::LockedCell;
        }
        // The live layout is laid out at the origin, in the cell's own local
        // frame: x runs 0..width across, y runs 0..-height down. The surface maps
        // that frame onto the cell's screen rectangle at the view's scale, so the
        // edited text sits exactly where the table draws its own.
        let (_, _, bottom, right) =
            crate::entities::table::containing_cell_span(table, row, column);
        let cell_width: f64 = table
            .columns
            .iter()
            .skip(column)
            .take(right + 1 - column)
            .map(|column| column.width)
            .sum();
        let cell_height: f64 = table
            .rows
            .iter()
            .skip(row)
            .take(bottom + 1 - row)
            .map(|row| row.height)
            .sum();
        if !cell_width.is_finite() || cell_width <= 0.0 || !(rect.width > 1.0) {
            return TableCellEditStart::NoCell;
        }
        let cell_world = [0.0, -cell_height as f32, cell_width as f32, 0.0];
        let original = table.cell_text(row, column).unwrap_or_default().to_string();
        // Edit the visible text, not the raw value: a cell's MText may carry
        // inline codes (`{\fArial|b1;…}`, `\P`, `\H2.5x;`) and the layout only
        // draws what the parser keeps. Putting the raw string in the sink would
        // count the caret in a different space than the glyphs it sits on.
        let (cells, para0, original_value) = self.cell_document(&original);
        let mut content = text_editor::Content::with_text(&super::mtext_editor::cells_text(&cells));
        // The caret opens after the cell's text, where typing continues it. Move
        // it through the editor's own motion: a hand-built line/column would be
        // counted in characters while the shaper indexes bytes, which panics on
        // any non-ASCII text (Cyrillic in a cell is the common case).
        content.perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
        let caret = cell_caret_vis(&content);
        // Blank the table's own text for this cell: the live surface is then the
        // only text there, with nothing painted over it. Deliberately outside
        // the undo history — an edit that is cancelled must leave no trace.
        // (A cell the picture never drew — an empty one — has nothing to blank;
        // the live surface then sits on top of whatever the table draws there.)
        let blanked = crate::entities::table::patch_block_cell_text(
            &mut self.tabs[i].scene.document,
            handle,
            row,
            column,
            "",
        );
        if blanked {
            self.tabs[i]
                .scene
                .bump_entities(&[(handle, crate::scene::ChangeKind::Modified)]);
            // Only the table's picture block changed: rebuilding every cached
            // block in the drawing is what made the double-click freeze.
            let block =
                crate::entities::table::picture_block_name(&self.tabs[i].scene.document, handle)
                    .unwrap_or_default();
            self.tabs[i].scene.invalidate_block_definition(&block);
        }
        self.table_cell_edit = Some(TableCellEditState {
            handle,
            row,
            column,
            tab_id: self.tabs[i].id,
            content,
            cells,
            para0,
            original_value,
            caret,
            caret_blink_on: true,
            original,
            blanked,
            wires: Vec::new(),
            boxes: Vec::new(),
            cell_world,
            extent: cell_world,
            ink: iced::Color::from_rgba(0.0, 0.0, 0.0, 1.0),
        });
        // Opening the cell puts the keyboard in it: the double-click came from the
        // drawing, not from the command line, so typing belongs to the cell until
        // the user clicks or types elsewhere.
        self.cell_keyboard = true;
        self.rebuild_table_cell_surface();
        TableCellEditStart::Started
    }

    /// Parse a cell's MText value into the flat cell list the editor works in.
    /// Returns the cells, the first paragraph's properties and the value they
    /// serialize back to (so an untouched cell compares equal and writes
    /// nothing).
    fn cell_document(
        &self,
        raw: &str,
    ) -> (
        Vec<super::mtext_editor::Cell>,
        codec::entities::mtext_format::ParagraphProperties,
        String,
    ) {
        use codec::entities::mtext_format::{parse_mtext, MTextParagraph};

        let mut doc = parse_mtext(raw, true);
        // `parse_mtext` drops a trailing empty paragraph; restore it so a cell
        // whose text ends with a break keeps it.
        if crate::entities::text_support::ends_with_break(raw) {
            doc.paragraphs.push(MTextParagraph::new());
        }
        let para0 = super::mtext_editor::doc_para0(&doc);
        let cells = super::mtext_editor::doc_to_cells(&doc);
        let value = cells_value(&cells, &para0);
        (cells, para0, value)
    }

    /// Re-lay the edited value with the cell's own style and re-derive the caret
    /// from the sink's cursor.
    pub(crate) fn rebuild_table_cell_surface(&mut self) {
        let Some(ed) = self.table_cell_edit.as_ref() else {
            return;
        };
        // Lay the live text out against the editor's own document, which is the
        // one whose picture block it blanked.
        let Some(i) = self.tabs.iter().position(|tab| tab.id == ed.tab_id) else {
            return;
        };
        let (handle, row, column) = (ed.handle, ed.row, ed.column);
        // The laid-out value is the cell list serialized back: it carries the
        // inline codes the sink never shows, so the live text matches what the
        // table will draw after the commit.
        let value = cells_value(&ed.cells, &ed.para0);
        let caret_vis = cell_caret_vis(&ed.content);
        let Some(text) = cell_run(&self.tabs[i].scene.document, handle, row, column, value) else {
            return;
        };
        let entity = EntityType::MText(Box::new(text.clone()));
        let bg = self.tabs[i].scene.bg_color;
        let color = text.common.color.clone();
        let color = match color {
            // Still `ByLayer`/`ByBlock` means nothing in the cell style chain
            // resolved a colour: draw the way CAD colour 7 does — dark ink on a
            // light sheet, light ink on a dark one — instead of a flat grey that
            // vanishes into white paper.
            codec::types::Color::ByLayer | codec::types::Color::ByBlock => {
                let sheet = self.tabs[i].scene.canvas_clear_color();
                let luma = 0.299 * sheet[0] + 0.587 * sheet[1] + 0.114 * sheet[2];
                if luma > 0.5 {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [1.0, 1.0, 1.0, 1.0]
                }
            }
            color => crate::scene::convert::tess_util::aci_to_rgba(&color),
        };
        let wires = crate::scene::convert::tessellate::tessellate(
            &self.tabs[i].scene.document,
            handle,
            &entity,
            false,
            color,
            0.0,
            [0.0; 8],
            1.0,
            1.0,
            None,
            None,
            bg,
            // A 2D canvas has no SDF shader: draw the glyph outline strokes.
            true,
        );
        let boxes = crate::entities::mtext::glyph_boxes(&text, &self.tabs[i].scene.document);
        if let Some(ed) = self.table_cell_edit.as_mut() {
            ed.wires = wires;
            // The caret index counts the sink's visible characters, the same
            // space the layout boxes live in, so it needs no remapping.
            ed.caret = caret_vis.min(boxes.len());
            ed.boxes = boxes;
            ed.caret_blink_on = true;
            ed.ink = iced::Color::from_rgba(color[0], color[1], color[2], 1.0);
            self.fit_table_cell_paint();
        }
    }

    /// Grow the surface's local frame to whatever the live run actually covers.
    /// Rotated cell text does not stay inside the cell's own rectangle (a
    /// vertical run reads upward past the top edge), and a canvas clips at its
    /// bounds: without this the end of a vertical run — and the caret sitting
    /// there — is cut away.
    fn fit_table_cell_paint(&mut self) {
        let Some(ed) = self.table_cell_edit.as_ref() else {
            return;
        };
        let f = ed.cell_world;
        let (mut minx, mut miny, mut maxx, mut maxy) = (f[0], f[1], f[2], f[3]);
        for wire in &ed.wires {
            for p in &wire.points {
                let (x, y) = (p[0], p[1]);
                if x.is_finite() && y.is_finite() {
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
        for b in &ed.boxes {
            for &(x, y) in &b.quad {
                minx = minx.min(x);
                miny = miny.min(y);
                maxx = maxx.max(x);
                maxy = maxy.max(y);
            }
        }
        let extent = [minx, miny, maxx, maxy];
        if let Some(ed) = self.table_cell_edit.as_mut() {
            ed.extent = extent;
        }
    }

    /// Put the sink's caret on a slot index — the numbering the glyph boxes and
    /// the caret use (a paragraph break owns one slot). iced's editor indexes a
    /// cursor by line and BYTE column, not by character, so the walk below counts
    /// bytes on purpose: a character count panics inside the shaper on any
    /// non-ASCII text.
    pub(crate) fn table_cell_set_caret(&mut self, slot: usize) {
        let Some(ed) = self.table_cell_edit.as_mut() else {
            return;
        };
        let text = ed.content.text();
        let (mut line, mut column, mut seen) = (0usize, 0usize, 0usize);
        for ch in text.chars() {
            if seen == slot {
                break;
            }
            if ch == '\n' {
                line += 1;
                column = 0;
            } else {
                column += ch.len_utf8();
            }
            seen += 1;
        }
        ed.content.move_to(text_editor::Cursor {
            position: text_editor::Position { line, column },
            selection: None,
        });
        self.rebuild_table_cell_surface();
    }

    /// Route one sink action: Enter commits the cell, everything else edits it.
    pub(crate) fn table_cell_edit_action(&mut self, action: text_editor::Action) {
        // Enter commits the cell; Shift+Enter types a line break. iced reports
        // both as the same action, so the modifier is tracked separately.
        if matches!(action, text_editor::Action::Edit(text_editor::Edit::Enter)) {
            if self.table_cell_shift {
                if let Some(ed) = self.table_cell_edit.as_mut() {
                    // Perform the editor's own line break, not a paste of "\n":
                    // pasting text leaves the editor's cursor on the line it was
                    // on, so the caret stayed behind until the next keystroke.
                    ed.content
                        .perform(text_editor::Action::Edit(text_editor::Edit::Enter));
                    let text = ed.content.text();
                    super::mtext_editor::cells_set_text(&mut ed.cells, &ed.para0, &text);
                }
                self.rebuild_table_cell_surface();
                return;
            }
            self.table_cell_commit();
            return;
        }
        let Some(ed) = self.table_cell_edit.as_mut() else {
            return;
        };
        ed.content.perform(action);
        // Keep the formatting cells in step with what the user typed: the diff
        // keeps every code the edit left alone and gives new text the formatting
        // in force at the caret.
        let text = ed.content.text();
        super::mtext_editor::cells_set_text(&mut ed.cells, &ed.para0, &text);
        self.rebuild_table_cell_surface();
    }

    /// Settle an open cell editor before anything reads or writes the document.
    /// The editor keeps the cell's text blanked in the picture block while it is
    /// open (that is how the live surface is the only text there), so a save that
    /// ran in the middle of an edit would write the cell away. Committing is the
    /// safe direction: an unchanged edit restores the original text and leaves the
    /// history alone.
    pub(crate) fn settle_table_cell_edit(&mut self) {
        if self.table_cell_edit.is_some() {
            self.table_cell_commit();
        }
    }

    /// Write the edited text into the cell and its block text, and close.
    pub(crate) fn table_cell_commit(&mut self) {
        let Some(ed) = self.table_cell_edit.take() else {
            return;
        };
        self.cell_keyboard = false;
        // Write into the document the editor was opened on, not whichever tab
        // happens to be active now.
        let Some(i) = self.tabs.iter().position(|tab| tab.id == ed.tab_id) else {
            return;
        };
        // Fold what the user typed into the formatting cells, then serialize —
        // the code the user never touched survives, so a bold or restyled cell
        // stays bold. A line break is a real `\n` in the sink (Shift+Enter or a
        // pasted multi-line text); the old `/n` command-line spelling is gone.
        let typed = ed.content.text();
        let mut cells = ed.cells.clone();
        super::mtext_editor::cells_set_text(&mut cells, &ed.para0, &typed);
        let value = cells_value(&cells, &ed.para0);
        if value == ed.original_value {
            // Nothing changed: put the table's own text back and leave the
            // history alone.
            self.restore_table_cell_text(&ed);
            self.refresh_properties();
            return;
        }
        // One step for the data and the block text together. The blank the
        // editor left is not part of the history: put the original back first,
        // so the snapshot's "before" is the state the user sees when they open
        // the cell — otherwise undo would restore the blank.
        {
            let doc = &mut self.tabs[i].scene.document;
            if ed.blanked {
                crate::entities::table::patch_block_cell_text(
                    doc,
                    ed.handle,
                    ed.row,
                    ed.column,
                    &ed.original,
                );
            }
        }
        self.push_undo_snapshot(i, "TABLE CELL");
        let doc = &mut self.tabs[i].scene.document;
        if let Some(EntityType::Table(table)) = doc.get_entity_mut(ed.handle) {
            if let Some(cell) = table.cell_mut(ed.row, ed.column) {
                cell.set_text(&value);
            }
        }
        crate::entities::table::patch_block_cell_text(doc, ed.handle, ed.row, ed.column, &value);
        self.tabs[i]
            .scene
            .bump_entities(&[(ed.handle, crate::scene::ChangeKind::Modified)]);
        // Only the block's own drawing changed: rebuilding every cached block in
        // the drawing made Enter feel like a freeze.
        let block =
            crate::entities::table::picture_block_name(&self.tabs[i].scene.document, ed.handle)
                .unwrap_or_default();
        self.tabs[i].scene.invalidate_block_definition(&block);
        self.tabs[i].dirty = true;
        self.finish_pending_history(i);
        self.refresh_properties();
    }

    /// Close without touching the cell: the editor never wrote to the drawing,
    /// so the authored text is still there.
    pub(crate) fn table_cell_cancel(&mut self) {
        self.cell_keyboard = false;
        if let Some(ed) = self.table_cell_edit.take() {
            self.restore_table_cell_text(&ed);
        }
        self.refresh_properties();
    }

    /// Put back the text the table drew for the edited cell. The editor blanks
    /// it while open (that is how the cell shows only the live text); undo never
    /// saw the blank, so neither cancelling nor an unchanged commit leaves a
    /// step behind. The cell belongs to the tab the editor opened in, which need
    /// not be the active tab any more — restore it where it lives.
    fn restore_table_cell_text(&mut self, ed: &TableCellEditState) {
        if !ed.blanked {
            return;
        }
        let Some(i) = self.tabs.iter().position(|tab| tab.id == ed.tab_id) else {
            // The document is gone: there is nothing left to put the text back
            // into, and patching another tab's document with this handle would
            // blank a cell in the wrong drawing.
            return;
        };
        let doc = &mut self.tabs[i].scene.document;
        crate::entities::table::patch_block_cell_text(
            doc,
            ed.handle,
            ed.row,
            ed.column,
            &ed.original,
        );
        self.tabs[i]
            .scene
            .bump_entities(&[(ed.handle, crate::scene::ChangeKind::Modified)]);
        let block =
            crate::entities::table::picture_block_name(&self.tabs[i].scene.document, ed.handle)
                .unwrap_or_default();
        self.tabs[i].scene.invalidate_block_definition(&block);
    }
}

/// The MText value a cell list stands for.
///
/// `to_mtext_string` drops a trailing empty paragraph, so a break typed at the
/// end of a cell — the everyday "Shift+Enter, now type on the second line" —
/// would vanish: the layout then has no slot for the new line, and the caret
/// sticks to the end of the first. Put the break back.
fn cells_value(
    cells: &[super::mtext_editor::Cell],
    para0: &codec::entities::mtext_format::ParagraphProperties,
) -> String {
    let mut value = super::mtext_editor::cells_to_doc(para0, cells).to_mtext_string();
    let trailing_break = matches!(cells.last(), Some(super::mtext_editor::Cell::Break(..)));
    if trailing_break && !crate::entities::text_support::ends_with_break(&value) {
        match value.strip_suffix('}') {
            Some(inner) => value = format!("{inner}\\P}}"),
            None => value.push_str("\\P"),
        }
    }
    value
}

/// The caret offset in the sink's text expressed against the laid-out glyphs:
/// characters before the cursor, minus the line breaks (a break is a new line,
/// not a glyph).
fn cell_caret_vis(content: &text_editor::Content) -> usize {
    let text = content.text();
    let cursor = content.cursor().position;
    // `column` is a BYTE offset into the line (the shaper's own index), not a
    // character count: counting characters there misplaces the caret on any
    // non-ASCII text.
    let mut bytes = 0;
    for (index, line) in text.split('\n').enumerate() {
        if index == cursor.line {
            bytes += cursor.column.min(line.len());
            break;
        }
        bytes += line.len() + 1;
    }
    let mut bytes = bytes.min(text.len());
    while bytes > 0 && !text.is_char_boundary(bytes) {
        bytes -= 1;
    }
    let before = &text[..bytes];
    // A paragraph break owns a slot in the layout's own numbering (see
    // `GlyphBox::vis`), so it counts here too — subtracting it put the caret one
    // slot behind on every line after a break.
    before.chars().count()
}

fn cell_run(
    document: &codec::CadDocument,
    handle: Handle,
    row: usize,
    column: usize,
    value: String,
) -> Option<MText> {
    let table = match document.get_entity(handle) {
        Some(EntityType::Table(table)) => table,
        _ => return None,
    };
    let (top, left, _, _) = crate::entities::table::containing_cell_span(table, row, column);
    let x0: f64 = table.columns.iter().take(left).map(|c| c.width).sum();
    let y0: f64 = table.rows.iter().take(top).map(|r| r.height).sum();
    // The colour the table draws this cell's text with: the content's own, else
    // the cell style chain. The picture's text is usually `ByLayer`, and taking
    // that literally draws the live text in a flat grey — invisible on a white
    // sheet, which is exactly how a table on paper looks.
    let cell_color: Option<[f32; 4]> = table
        .rows
        .get(row)
        .and_then(|row_data| {
            Some((
                row_data,
                row_data.cells.get(column)?,
                row_data.cells[column].contents.first()?,
            ))
        })
        .map(|(row_data, cell, content)| {
            crate::entities::table::cell_content_text_format(
                document, table, row_data, row, column, cell, content,
            )
            .color
        });
    // The text the picture block actually draws for this cell is the single
    // source of truth: it carries the authored style, height, rotation and
    // attachment. Re-deriving them from the cell data loses whatever lives
    // only in the picture (a vertical cell's rotation, for one).
    if let Some(EntityType::MText(source)) =
        crate::entities::table::block_cell_text_target(document, handle, row, column)
            .and_then(|target| document.get_entity(target))
    {
        let mut text = (**source).clone();
        text.value = value;
        // Block coordinates are table-wide: shift the insertion into the
        // cell's own frame, where x = 0 is the cell's left edge and y = 0 its
        // top.
        text.insertion_point.x -= x0;
        text.insertion_point.y += y0;
        if matches!(
            text.common.color,
            codec::types::Color::ByLayer | codec::types::Color::ByBlock
        ) {
            if let Some([r, g, b, _]) = cell_color {
                text.common.color = rgb_color(r, g, b);
            }
        }
        return Some(text);
    }
    // An empty cell the picture never drew: build the run from the cell style.
    let row_data = table.rows.get(row)?;
    let cell = row_data.cells.get(column)?;
    let content = cell.contents.first()?;
    let format = crate::entities::table::cell_content_text_format(
        document, table, row_data, row, column, cell, content,
    );
    let mut text = MText::new();
    text.value = value;
    text.style = format.style;
    text.height = format.height;
    text.rotation = format.rotation;
    text.rectangle_width = format.wrap_width;
    text.insertion_point = codec::types::Vector3::new(format.anchor.0, format.anchor.1, 0.0);
    text.attachment_point = format.attachment;
    text.common.color = match format.color {
        [r, g, b, _] => rgb_color(r, g, b),
    };
    Some(text)
}

/// A true-colour value from 0..1 components.
fn rgb_color(r: f32, g: f32, b: f32) -> codec::types::Color {
    codec::types::Color::Rgb {
        r: (r.clamp(0.0, 1.0) * 255.0).round() as u8,
        g: (g.clamp(0.0, 1.0) * 255.0).round() as u8,
        b: (b.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}
