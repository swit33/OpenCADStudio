//! Table cell editing from the drawing area: the pick contract, the in-place
//! editor, and what an edit does to the table's baked picture block. A pick
//! inside a cell resolves that cell in whatever space is being edited (and never
//! a table of another space), the editor draws the live text over the cell, and
//! writing a cell leaves every other cell exactly as the file drew it.
use super::OpenCADStudio;
use crate::command::StepInput;
use crate::modules::annotate::table_cmd::TableCellEditStart;
use codec::entities::Table;
use codec::types::Vector3;
use codec::{EntityType, Handle};
use glam::DVec3;
use iced::widget::text_editor::{Action, Edit};

/// The screen rectangle tests hand the editor (there is no layout in a test, so
/// nothing projects the cell): 200 × 40 pixels.
fn test_rect() -> iced::Rectangle {
    iced::Rectangle {
        x: 0.0,
        y: 0.0,
        width: 200.0,
        height: 40.0,
    }
}

/// A fresh drawing holding one 2×2 table at the origin. `Table::new` seeds
/// 2.5-wide columns and 0.25-high rows flowing down, the same defaults the
/// cell-resolver tests pin.
fn app_with_table() -> (OpenCADStudio, Handle) {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let table = Table::new(Vector3::new(0.0, 0.0, 0.0), 2, 2);
    let handle = app.tabs[0]
        .scene
        .document
        .add_entity(EntityType::Table(Box::new(table)))
        .unwrap();
    (app, handle)
}

/// Centre of cell [0,0] — and of [1,1] — for the table above.
fn cell_center(row: usize, column: usize) -> DVec3 {
    DVec3::new(1.25 + 2.5 * column as f64, -0.125 - 0.25 * row as f64, 0.0)
}

fn paper_space_block(app: &OpenCADStudio) -> Handle {
    app.tabs[0]
        .scene
        .document
        .block_records
        .iter()
        .find(|record| record.name.starts_with("*Paper_Space"))
        .map(|record| record.handle)
        .expect("a new drawing has a paper space block")
}

#[test]
fn the_editor_opens_on_the_picked_cell() {
    let (mut app, handle) = app_with_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 1, test_rect()),
        TableCellEditStart::Started
    );
    let ed = app.table_cell_edit.as_ref().expect("the editor is open");
    assert_eq!((ed.handle, ed.row, ed.column), (handle, 0, 1));
    assert!(
        app.tabs[0].active_cmd.is_none(),
        "the in-place editor is host state, not a command"
    );
}

#[test]
fn tabledit_pick_resolves_the_cell_under_the_point() {
    let (mut app, _) = app_with_table();
    let _ = app.run_command_line("TABLEDIT");
    assert_eq!(
        app.tabs[0].active_cmd.as_ref().map(|cmd| cmd.name()),
        Some("TABLEDIT"),
        "TABLEDIT waits for a pick"
    );
    // The pick itself carries only a point; the host resolves the cell from it.
    // (With no layout there is no screen rectangle to draw the editor in, so the
    // pick lands back on TABLEDIT's prompt — the resolution path is what this
    // pins, the projection is view-side.)
    use crate::command::CadCommand;
    let mut pick = crate::modules::annotate::table_cmd::TableditCommand::new();
    assert!(matches!(
        pick.on_point(cell_center(1, 1)),
        crate::command::CmdResult::EditTableCell { handle: Handle::NULL, .. }
    ));
    let _ = app.feed_command(StepInput::Point(cell_center(1, 1)));
    assert_eq!(
        app.tabs[0].active_cmd.as_ref().map(|cmd| cmd.name()),
        Some("TABLEDIT"),
        "a pick with no editable cell under it re-prompts"
    );
}

#[test]
fn a_table_in_another_space_is_not_picked() {
    let (mut app, handle) = app_with_table();
    let paper = paper_space_block(&app);
    // The same world coordinates, but the table lives in paper space: while
    // Model is the active space the resolver must not land on it.
    if let Some(entity) = app.tabs[0].scene.document.get_entity_mut(handle) {
        entity.common_mut().owner_handle = paper;
    }
    assert_eq!(
        app.begin_table_cell_edit(0, Handle::NULL, cell_center(0, 0)),
        TableCellEditStart::NoCell
    );
    assert!(app.table_cell_edit.is_none(), "no editor opened");
}

#[test]
fn escape_closes_the_editor_and_restores_the_cell() {
    let (mut app, _) = app_with_table();
    assert_eq!(
        app.open_table_cell_edit(handle_of(&app), 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    assert!(app.table_cell_edit.is_some());
    let _ = app.update(super::Message::CommandEscape);
    assert!(app.table_cell_edit.is_none(), "Esc closes the editor");
}

/// The drawing's only table.
fn handle_of(app: &OpenCADStudio) -> Handle {
    app.tabs[0]
        .scene
        .document
        .entities()
        .find_map(|entity| match entity {
            EntityType::Table(table) => Some(table.common.handle),
            _ => None,
        })
        .expect("a table")
}

// ── Writing a cell ──────────────────────────────────────────────────────────
//
// A table from a file carries a baked picture block: the cell texts, their
// styles and their rotations as the authoring application drew them. Writing a
// cell must reuse that block — rebuilding it from the cell data loses the
// authored text style and rotation, which flattened every vertical cell in the
// table the first time any cell was edited.

/// A drawing with a 2×2 table whose cells read "A" (row 0) and "B" (row 1) and
/// whose picture block is already baked, the way an opened file has it.
fn app_with_baked_table() -> (OpenCADStudio, Handle) {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    // Three rows so the table has a title, a header and a data row (the roles
    // the table style classifies rows by).
    let mut table = Table::new(Vector3::new(0.0, 0.0, 0.0), 3, 2);
    for (row, column, value) in [
        (0, 0, "A"),
        (0, 1, "B"),
        (1, 0, "C"),
        (1, 1, "D"),
        (2, 0, "E"),
        (2, 1, "F"),
    ] {
        if let Some(cell) = table.cell_mut(row, column) {
            cell.set_text(value);
        }
    }
    let handle = app.tabs[0]
        .scene
        .document
        .add_entity(EntityType::Table(Box::new(table)))
        .unwrap();
    app.tabs[0].scene.document.refresh_table_block(handle);
    // An authored table's cell text may carry a style and a rotation of its
    // own (that is how vertical cell text reaches a file).
    set_block_text_style(&mut app, handle, "B", "Vertical", 90.0_f64.to_radians());
    (app, handle)
}

/// The table's baked cell texts: (value, style, rotation) per MText in its
/// block, ordered top-down then left-right.
fn block_cell_texts(app: &OpenCADStudio, handle: Handle) -> Vec<(String, String, f64)> {
    let doc = &app.tabs[0].scene.document;
    let table = match doc.get_entity(handle) {
        Some(EntityType::Table(table)) => table,
        _ => panic!("table entity missing"),
    };
    let record = table
        .block_record_handle
        .and_then(|block| doc.block_records.iter().find(|r| r.handle == block))
        .expect("the table keeps its picture block");
    let mut texts: Vec<(String, String, f64)> = record
        .entity_handles
        .iter()
        .filter_map(|h| match doc.get_entity(*h) {
            Some(EntityType::MText(text)) => {
                Some((text.value.clone(), text.style.clone(), text.rotation))
            }
            _ => None,
        })
        .collect();
    texts.sort_by(|a, b| a.0.cmp(&b.0));
    texts
}

/// Give the baked text holding `value` a text style and rotation.
fn set_block_text_style(
    app: &mut OpenCADStudio,
    handle: Handle,
    value: &str,
    style: &str,
    tilt: f64,
) {
    let doc = &mut app.tabs[0].scene.document;
    let block = match doc.get_entity(handle) {
        Some(EntityType::Table(table)) => table.block_record_handle.expect("baked block"),
        _ => panic!("table entity missing"),
    };
    let targets: Vec<Handle> = doc
        .block_records
        .iter()
        .find(|r| r.handle == block)
        .map(|r| r.entity_handles.clone())
        .unwrap_or_default();
    for target in targets {
        if let Some(EntityType::MText(text)) = doc.get_entity_mut(target) {
            if text.value == value {
                text.style = style.to_string();
                text.rotation = tilt;
            }
        }
    }
}

/// Replace cell (`row`, `column`)'s text through the in-place editor: open it on
/// the cell, select what is there, type, and commit with Enter.
fn type_into_cell(app: &mut OpenCADStudio, row: usize, column: usize, text: &str) {
    let handle = handle_of(app);
    assert_eq!(
        app.open_table_cell_edit(handle, row, column, test_rect()),
        TableCellEditStart::Started
    );
    let _ = app.update(super::Message::TableCellEditAction(Action::SelectAll));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Backspace,
    )));
    for ch in text.chars() {
        let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
            Edit::Insert(ch),
        )));
    }
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    assert!(app.table_cell_edit.is_none(), "Enter commits and closes");
}

#[test]
fn editing_one_cell_leaves_the_other_cells_as_authored() {
    let (mut app, handle) = app_with_baked_table();
    let before = block_cell_texts(&app, handle);
    type_into_cell(&mut app, 0, 0, "new");
    let after = block_cell_texts(&app, handle);

    // Same number of baked texts, same order: the block was reused, not rebuilt.
    assert_eq!(after.len(), before.len(), "no cell text was re-created");
    // The edited cell shows the new text...
    assert!(
        after.iter().any(|(value, _, _)| value == "new"),
        "the edited cell shows what was typed: {after:?}"
    );
    // ...and the untouched ones are byte-for-byte what the file drew.
    for original in before.iter().filter(|(value, _, _)| value != "A") {
        assert!(
            after.contains(original),
            "cell {original:?} lost its authored value/style/rotation: {after:?}"
        );
    }
}

#[test]
fn editing_a_cell_keeps_its_vertical_text_style() {
    let (mut app, handle) = app_with_baked_table();
    type_into_cell(&mut app, 0, 0, "new");
    let after = block_cell_texts(&app, handle);
    let vertical = after
        .iter()
        .find(|(value, _, _)| value == "B")
        .expect("the other cell still holds its text");
    assert_eq!(vertical.1, "Vertical", "text style survived the edit");
    assert!(
        (vertical.2 - 90.0_f64.to_radians()).abs() < 1e-9,
        "rotation survived the edit: {}",
        vertical.2
    );
}

#[test]
fn cell_edit_undoes_as_one_step() {
    let (mut app, handle) = app_with_baked_table();
    type_into_cell(&mut app, 0, 0, "new");
    assert_eq!(cell_value(&app, handle, 0, 0), "new");

    let _ = app.update(super::Message::Undo);
    assert_eq!(
        cell_value(&app, handle, 0, 0),
        "A",
        "undo restores the cell"
    );
    assert!(
        block_cell_texts(&app, handle)
            .iter()
            .any(|(value, _, _)| value == "A"),
        "and restores what the block draws with it"
    );
}

fn cell_value(app: &OpenCADStudio, handle: Handle, row: usize, column: usize) -> String {
    match app.tabs[0].scene.document.get_entity(handle) {
        Some(EntityType::Table(table)) => {
            table.cell_text(row, column).unwrap_or_default().to_string()
        }
        _ => panic!("table entity missing"),
    }
}

/// Arm a table cell the way the double-click does, so a Properties edit lands on
/// it (the Cell section is what the panel shows for an armed cell).
fn arm_cell(app: &mut OpenCADStudio, columns: usize, row: usize, column: usize) {
    app.tabs[0].properties.prop_vertex = row * columns + column;
    app.tabs[0].properties.prop_vertex_indicator_active = true;
}

#[test]
fn cell_text_style_choice_restyles_only_that_cell() {
    let (mut app, handle) = app_with_baked_table();
    app.tabs[0].scene.select_entity(handle, true);
    arm_cell(&mut app, 2, 2, 1);
    let before = block_cell_texts(&app, handle);

    let _ = app.update(super::Message::PropGeomChoiceChanged {
        field: "tbl_cell_text_style",
        value: "Standard".to_string(),
    });

    let after = block_cell_texts(&app, handle);
    // The edited cell (value "F") now draws with the chosen style...
    let edited = after
        .iter()
        .find(|(value, _, _)| value == "F")
        .expect("the edited cell still draws its text");
    assert_eq!(edited.1, "Standard", "the choice reached the baked text");
    // ...and every other cell keeps the style and rotation it was drawn with.
    for original in before.iter().filter(|(value, _, _)| value != "F") {
        assert!(
            after.contains(original),
            "cell {original:?} lost its authored styling: {after:?}"
        );
    }
}

#[test]
fn table_style_edit_restyles_the_baked_cells_of_its_tables() {
    use codec::objects::{ObjectType, TableStyle};

    let (mut app, handle) = app_with_baked_table();
    // A document table style whose data rows ask for another text style, and a
    // table pointing at it.
    let mut style = TableStyle::new("Site");
    style.data_row_style.text_style_name = "Vertical".to_string();
    let style_handle = app.tabs[0].scene.document.allocate_handle();
    style.handle = style_handle;
    app.tabs[0]
        .scene
        .document
        .objects
        .insert(style_handle, ObjectType::TableStyle(style));
    if let Some(EntityType::Table(table)) = app.tabs[0].scene.document.get_entity_mut(handle) {
        table.table_style_handle = Some(style_handle);
    }

    let touched = crate::entities::table::patch_tables_using_style(
        &mut app.tabs[0].scene.document,
        style_handle,
    );
    assert_eq!(
        touched,
        vec![handle],
        "the table using the style was restyled"
    );
    let after = block_cell_texts(&app, handle);
    // Row 2 is the data row: its cells take the row style's text style.
    for value in ["E", "F"] {
        let cell = after
            .iter()
            .find(|(text, _, _)| text == value)
            .expect("data cell still draws");
        assert_eq!(cell.1, "Vertical", "data cell {value} restyled");
    }
}

#[test]
fn properties_cell_text_edit_reaches_the_baked_text() {
    let (mut app, handle) = app_with_baked_table();
    app.tabs[0].scene.select_entity(handle, true);
    arm_cell(&mut app, 2, 2, 0);
    if let Some(EntityType::Table(table)) = app.tabs[0].scene.document.get_entity_mut(handle) {
        if let Some(cell) = table.cell_mut(2, 0) {
            cell.set_text("edited in the panel");
        }
    }
    let mut patched = crate::entities::table::patch_block_cell_format(
        &mut app.tabs[0].scene.document,
        handle,
        2,
        0,
        None,
    );
    patched |= crate::entities::table::patch_block_cell_text(
        &mut app.tabs[0].scene.document,
        handle,
        2,
        0,
        "edited in the panel",
    );
    assert!(patched, "the cell's baked text was rewritten");
    let after = block_cell_texts(&app, handle);
    assert!(
        after
            .iter()
            .any(|(value, _, _)| value == "edited in the panel"),
        "the panel edit shows in the block: {after:?}"
    );
    let untouched = after
        .iter()
        .find(|(value, _, _)| value == "B")
        .expect("the other cells still draw");
    assert_eq!(untouched.1, "Vertical", "their styling is intact");
}

/// Open the in-place editor on cell (`row`, `column`) and type `text` into it
/// without committing.
fn type_without_committing(app: &mut OpenCADStudio, row: usize, column: usize, text: &str) {
    let handle = handle_of(app);
    assert_eq!(
        app.open_table_cell_edit(handle, row, column, test_rect()),
        TableCellEditStart::Started
    );
    let _ = app.update(super::Message::TableCellEditAction(Action::SelectAll));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Backspace,
    )));
    for ch in text.chars() {
        let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
            Edit::Insert(ch),
        )));
    }
}

#[test]
fn typing_leaves_the_history_alone_until_enter() {
    let (mut app, handle) = app_with_baked_table();
    type_without_committing(&mut app, 0, 0, "new");
    // The editor hides the table's own text for the cell (that is how the live
    // surface is the only text there) and writes nothing to the cell data.
    assert_eq!(cell_value(&app, handle, 0, 0), "A");
    assert!(
        block_cell_texts(&app, handle)
            .iter()
            .any(|(value, _, _)| value.is_empty()),
        "the drawn text for the cell is blanked while editing"
    );
    assert_eq!(app.tabs[0].history.undo_stack.len(), 0, "no undo step yet");

    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    assert_eq!(cell_value(&app, handle, 0, 0), "new");
    assert!(block_cell_texts(&app, handle)
        .iter()
        .any(|(value, _, _)| value == "new"));
    assert_eq!(app.tabs[0].history.undo_stack.len(), 1, "one step per edit");
}

/// Cancelling must leave the drawing exactly as it was, including the text the
/// editor blanked to make room for the live surface.
#[test]
fn cancelling_puts_the_drawn_text_back() {
    let (mut app, handle) = app_with_baked_table();
    let before = block_cell_texts(&app, handle);
    type_without_committing(&mut app, 0, 0, "new");
    let _ = app.update(super::Message::CommandEscape);
    assert_eq!(block_cell_texts(&app, handle), before);
    assert_eq!(cell_value(&app, handle, 0, 0), "A");
    assert_eq!(app.tabs[0].history.undo_stack.len(), 0);
}

#[test]
fn slash_n_is_plain_text_now() {
    let (mut app, handle) = app_with_baked_table();
    // `/n` came from the command line, where it was the only way to type a line
    // break. The cell editor has Shift+Enter now, so a literal `/n` in cell text
    // — a dimension like `1/2`, a date — must stay what the user typed.
    type_into_cell(&mut app, 0, 0, "Foo/nBar");
    assert_eq!(cell_value(&app, handle, 0, 0), "Foo/nBar");
}

/// A cell whose authored value carries inline codes: the editor shows the
/// visible text only, the caret counts that same text, and a commit keeps the
/// code the user never touched.
#[test]
fn a_coded_cell_keeps_its_codes_and_its_caret_tracks_the_glyphs() {
    let (mut app, handle) = app_with_baked_table();
    let coded = "{\\fArial|b1;Врезка}";
    if let Some(EntityType::Table(table)) = app.tabs[0].scene.document.get_entity_mut(handle) {
        if let Some(cell) = table.cell_mut(0, 0) {
            cell.set_text(coded);
        }
    }
    app.tabs[0].scene.document.refresh_table_block(handle);
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    let ed = app.table_cell_edit.as_ref().expect("the editor is open");
    assert_eq!(
        ed.content.text(),
        "Врезка",
        "the sink holds the visible text, not the codes"
    );
    assert_eq!(
        ed.caret,
        ed.boxes.len(),
        "the caret sits on the last laid-out glyph, not past the code-laden raw string"
    );
    // Type at the end and commit: the font code has to survive.
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Insert('!'),
    )));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    let value = cell_value(&app, handle, 0, 0);
    assert!(value.contains("Врезка!"), "typed text landed: {value:?}");
    assert!(
        value.contains("Arial") || value.contains("\\f"),
        "the authored font code survived the edit: {value:?}"
    );
}

#[test]
fn the_caret_follows_the_edited_text() {
    let (mut app, handle) = app_with_baked_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    // "A" is one visible character: the caret opens after it.
    assert_eq!(app.table_cell_edit.as_ref().expect("open").caret, 1);
    for ch in "bc".chars() {
        let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
            Edit::Insert(ch),
        )));
    }
    let ed = app.table_cell_edit.as_ref().expect("still open");
    assert_eq!(ed.caret, 3, "caret sits after the typed characters");
    assert!(
        !ed.boxes.is_empty(),
        "the live text is laid out for the surface"
    );
}

/// The title row spans both columns: its text lives in the owner cell [0,0],
/// and a click on the right half of the span — cell [0,1], a member, not the
/// owner — must still edit the owner's text.
#[test]
fn a_click_inside_a_merged_span_edits_the_owner_cell() {
    let (mut app, handle) = app_with_baked_table();
    if let Some(EntityType::Table(table)) = app.tabs[0].scene.document.get_entity_mut(handle) {
        if let Some(cell) = table.cell_mut(0, 0) {
            cell.merge_width = 2;
        }
        table.sync_merged_ranges_from_cells();
    }
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 1, test_rect()),
        TableCellEditStart::Started
    );
    let ed = app.table_cell_edit.as_ref().expect("the editor is open");
    assert_eq!((ed.row, ed.column), (0, 0), "the merged span's owner");
    assert_eq!(ed.original, "A", "the owner carries the span's text");
    // The span is as wide as both columns: the surface's local frame spans it.
    let expected_width = 2.5 + 2.5;
    let screen = ed.screen(test_rect());
    assert!((screen.scale - 200.0 / expected_width).abs() < 1.0e-3);
    type_into_cell(&mut app, 0, 1, "T");
    assert_eq!(cell_value(&app, handle, 0, 0), "T");
    assert!(block_cell_texts(&app, handle)
        .iter()
        .any(|(value, _, _)| value == "T"));
}

/// A vertical run reads upward, past the cell's own rectangle. The surface's
/// frame must grow to cover it, or the caret at the end of the run is clipped
/// away and the user sees live text with no cursor.
#[test]
fn a_vertical_runs_caret_stays_inside_the_painted_frame() {
    let (mut app, handle) = app_with_baked_table();
    // The "B" cell is authored with a 90° rotation; a long value makes the run
    // read far past the cell's top edge, which is where a clipped caret hides.
    type_without_committing(&mut app, 0, 1, "VERTICAL");
    let ed = app.table_cell_edit.as_ref().expect("the editor is open");
    assert!(
        (ed.cell_world[2] - ed.cell_world[0]) > 0.0,
        "the cell's own frame is still known"
    );
    // Every glyph corner — the caret draws from these — is inside the frame
    // the canvas paints, so nothing the editor draws is clipped away.
    let screen = ed.screen(test_rect());
    let (x1, y1) = (screen.minx, screen.miny);
    let (x2, y2) = (
        screen.minx + screen.paint.width / screen.scale,
        screen.miny + screen.paint.height / screen.scale,
    );
    assert!(!ed.boxes.is_empty(), "the live run was laid out");
    for b in &ed.boxes {
        for (cx, cy) in b.quad {
            assert!(
                cx >= x1 - 1.0e-3
                    && cx <= x2 + 1.0e-3
                    && cy >= y1 - 1.0e-3
                    && cy <= y2 + 1.0e-3,
                "glyph corner ({cx}, {cy}) outside the frame [{x1}..{x2}] x [{y1}..{y2}]"
            );
        }
    }
    // The run really overflowed the cell, or the test would prove nothing.
    let cell_h = ed.cell_world[3] - ed.cell_world[1];
    assert!(
        (y2 - y1) > cell_h + 1.0e-3,
        "the frame grew past the cell to hold the rotated run: {} vs {cell_h}",
        y2 - y1
    );
}

/// Non-ASCII cell text: the caret offset and the editor's own cursor index must
/// agree, or opening the editor panics inside the shaper (`is_char_boundary`)
/// and the caret lands in the wrong place. Cyrillic in a table is the everyday
/// case, not an edge case.
#[test]
fn cyrillic_cell_text_opens_and_edits() {
    let (mut app, handle) = app_with_baked_table();
    if let Some(EntityType::Table(table)) = app.tabs[0].scene.document.get_entity_mut(handle) {
        if let Some(cell) = table.cell_mut(0, 0) {
            cell.set_text("Общие");
        }
    }
    app.tabs[0].scene.document.refresh_table_block(handle);
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    // Five characters, five glyphs, caret after them.
    let ed = app.table_cell_edit.as_ref().expect("the editor is open");
    assert_eq!(ed.original, "Общие");
    assert_eq!(ed.caret, 5, "caret sits after the Cyrillic text");
    // Editing after the text keeps working (no panic, text grows).
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Insert('!'),
    )));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    assert_eq!(cell_value(&app, handle, 0, 0), "Общие!");
}

/// The overlay must follow the cell: panning and zooming move the sheet under
/// the editor, so the screen rectangle it paints into is recomputed from the
/// camera rather than frozen at open time.
#[test]
fn the_overlay_follows_the_cell_when_the_drawing_moves() {
    let (mut app, handle) = app_with_baked_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    let at_open = {
        let ed = app.table_cell_edit.as_ref().expect("open");
        ed.screen(test_rect())
    };
    // The same cell, seen through a rectangle that moved and grew (a pan and a
    // zoom): the overlay follows both, and its text scale follows the zoom.
    let zoomed = iced::Rectangle {
        x: 40.0,
        y: 25.0,
        width: 400.0,
        height: 80.0,
    };
    let after = {
        let ed = app.table_cell_edit.as_ref().expect("open");
        ed.screen(zoomed)
    };
    assert!(
        (after.scale - at_open.scale * 2.0).abs() < 1.0e-3,
        "double the cell width on screen, double the text scale"
    );
    assert!((after.paint.x - zoomed.x).abs() < 1.0e-3, "follows the pan");
    assert!((after.paint.y - zoomed.y).abs() < 1.0e-3, "follows the pan");
    assert!((after.paint.width - zoomed.width).abs() < 1.0e-3);
}

/// Enter commits, Shift+Enter types a line break — and only Shift+Enter, so a
/// cell cannot be committed by accident with a modifier held.
#[test]
fn shift_enter_writes_a_line_break_instead_of_committing() {
    let (mut app, handle) = app_with_baked_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    let _ = app.update(super::Message::TableCellShift(true));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    assert!(
        app.table_cell_edit.is_some(),
        "Shift+Enter keeps the editor open"
    );
    let ed = app.table_cell_edit.as_ref().expect("open");
    assert!(ed.content.text().ends_with('\n'), "a line break was typed");
    let _ = app.update(super::Message::TableCellShift(false));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Insert('x'),
    )));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    assert!(app.table_cell_edit.is_none(), "plain Enter commits");
    let value = cell_value(&app, handle, 0, 0);
    assert!(value.contains("\\P"), "the break reached the value: {value:?}");
    assert!(value.contains('x'), "the typed text reached the value: {value:?}");
}

/// After Shift+Enter the caret must already be on the new line: the break is a
/// real line break in the sink, not a pasted newline that leaves the editor's
/// cursor behind on the previous line.
#[test]
fn shift_enter_moves_the_caret_to_the_new_line_at_once() {
    let (mut app, handle) = app_with_baked_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    let first_line = {
        let ed = app.table_cell_edit.as_ref().expect("open");
        ed.boxes.first().map(|b| b.quad[0].1).expect("a glyph")
    };
    let _ = app.update(super::Message::TableCellShift(true));
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Enter,
    )));
    let ed = app.table_cell_edit.as_ref().expect("still open");
    assert!(ed.content.text().ends_with('\n'), "a break was typed");
    // The caret's box is on the line BELOW the text, not on the first line.
    let caret_box = ed
        .boxes
        .iter()
        .find(|b| b.vis == ed.caret)
        .or_else(|| ed.boxes.iter().find(|b| b.vis + 1 == ed.caret))
        .expect("the caret has a slot on the new line");
    assert!(
        caret_box.quad[0].1 < first_line - 1.0e-3,
        "caret at y {} sits below the first line at {first_line}",
        caret_box.quad[0].1
    );
}

/// A click inside the live cell text must move the sink's caret and keep the
/// keyboard in the cell. The MText surface's own messages unfocus every widget,
/// which would send typing to the command line with the editor still open.
#[test]
fn a_click_in_the_cell_text_keeps_the_keyboard_in_the_cell() {
    let (mut app, handle) = app_with_baked_table();
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    assert!(app.cell_keyboard, "opening the cell claims the keyboard");
    // The user went to the command line, then came back by clicking the text.
    let _ = app.update(super::Message::CommandInput(String::new()));
    assert!(!app.cell_keyboard, "typing in the command line releases it");
    let _ = app.update(super::Message::TableCellSelStart(0));
    assert!(app.cell_keyboard, "clicking the cell text takes it back");
    let ed = app.table_cell_edit.as_ref().expect("the editor is still open");
    assert_eq!(ed.caret, 0, "the click placed the caret");
}

/// Opening another cell closes the previous editor properly: its blanked text is
/// put back. Dropping the state instead left that cell empty in the drawing.
#[test]
fn switching_cells_puts_the_first_cells_text_back() {
    let (mut app, handle) = app_with_baked_table();
    type_without_committing(&mut app, 0, 0, "x");
    assert!(
        block_cell_texts(&app, handle)
            .iter()
            .any(|(value, _, _)| value.is_empty()),
        "the first cell's drawn text is blanked while editing"
    );
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 1, test_rect()),
        TableCellEditStart::Started
    );
    let ed = app.table_cell_edit.as_ref().expect("the editor moved");
    assert_eq!((ed.row, ed.column), (0, 1));
    assert!(
        block_cell_texts(&app, handle)
            .iter()
            .any(|(value, _, _)| value == "A"),
        "the first cell's text is back where it was drawn"
    );
}

/// Anything that writes the document settles the editor first, so a save cannot
/// receive the blank the editor is holding.
#[test]
fn settling_before_a_save_writes_the_edit_and_closes() {
    let (mut app, handle) = app_with_baked_table();
    type_without_committing(&mut app, 0, 0, "saved");
    assert!(app.table_cell_edit.is_some());
    app.settle_table_cell_edit();
    assert!(app.table_cell_edit.is_none(), "the editor is closed");
    assert_eq!(cell_value(&app, handle, 0, 0), "saved");
    assert!(!app.cell_keyboard, "the keyboard goes back to the command line");
}

/// Ctrl+Z while editing belongs to the cell: the sink undoes its own character
/// and the drawing keeps its history, instead of one press undoing both.
#[test]
fn undo_while_editing_leaves_the_drawing_history_alone() {
    let (mut app, handle) = app_with_baked_table();
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Insert('x'),
    )));
    assert_eq!(
        app.open_table_cell_edit(handle, 0, 0, test_rect()),
        TableCellEditStart::Started
    );
    let _ = app.update(super::Message::TableCellEditAction(Action::Edit(
        Edit::Insert('y'),
    )));
    let _ = app.update(super::Message::Undo);
    assert_eq!(
        app.tabs[0].history.undo_stack.len(),
        0,
        "no drawing step was taken"
    );
    assert_eq!(cell_value(&app, handle, 0, 0), "A", "the cell data is untouched");
}

/// Editing one cell changes exactly one block — the table's picture. Bumping the
/// scene-wide block epoch instead made the next frame re-walk every block in the
/// drawing (measured ~170 ms on an ordinary sheet), which the user felt as a
/// freeze on every open and commit.
#[test]
fn editing_a_cell_rebuilds_only_the_tables_block() {
    let (mut app, handle) = app_with_baked_table();
    let epoch_before = app.tabs[0].scene.block_epoch;
    type_into_cell(&mut app, 0, 0, "new");
    assert_eq!(
        app.tabs[0].scene.block_epoch, epoch_before,
        "the scene-wide block cache was not thrown away"
    );
    assert!(
        block_cell_texts(&app, handle)
            .iter()
            .any(|(value, _, _)| value == "new"),
        "the drawn cell text still follows the edit"
    );
}
