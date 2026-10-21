//! Table cell editing from the drawing area. The double-click shortcut and the
//! TABLEDIT point pick both route through `begin_table_cell_edit`, so these
//! tests pin that shared contract: a pick inside a cell opens the cell editor
//! in whatever space is being edited, the editor marks the cell it opened (the
//! viewport outlines it from that mark), and a table belonging to another space
//! is never picked by coordinates alone.
use super::OpenCADStudio;
use crate::command::StepInput;
use crate::modules::annotate::table_cmd::{TableCellEditMark, TableCellEditStart};
use codec::entities::Table;
use codec::types::Vector3;
use codec::{EntityType, Handle};
use glam::DVec3;

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
fn pick_on_a_cell_opens_the_cell_editor() {
    let (mut app, handle) = app_with_table();
    // A pick from the middle of the cell (what a double-click on the cell body
    // resolves to: no wire under the cursor, so the grid answers).
    assert_eq!(
        app.begin_table_cell_edit(0, Handle::NULL, cell_center(0, 0)),
        TableCellEditStart::Started
    );
    let active = app.tabs[0]
        .active_cmd
        .as_ref()
        .expect("the cell editor is active");
    assert_eq!(active.name(), "TABLE CELL");
    assert!(
        active.prompt().contains("[0,0]"),
        "prompt names the picked cell: {}",
        active.prompt()
    );
    assert_eq!(
        app.table_cell_edit,
        Some(TableCellEditMark {
            handle,
            row: 0,
            column: 0,
        })
    );
}

#[test]
fn tabledit_point_pick_opens_the_cell_editor() {
    let (mut app, _) = app_with_table();
    let _ = app.run_command_line("TABLEDIT");
    assert_eq!(
        app.tabs[0].active_cmd.as_ref().map(|cmd| cmd.name()),
        Some("TABLEDIT"),
        "TABLEDIT waits for a pick"
    );
    let _ = app.feed_command(StepInput::Point(cell_center(1, 1)));
    let active = app.tabs[0]
        .active_cmd
        .as_ref()
        .expect("the pick handed over to the cell editor");
    assert_eq!(active.name(), "TABLE CELL");
    assert!(active.prompt().contains("[1,1]"), "{}", active.prompt());
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
    assert!(app.table_cell_edit.is_none());
}

#[test]
fn ending_the_cell_editor_drops_the_cell_mark() {
    let (mut app, _) = app_with_table();
    assert_eq!(
        app.begin_table_cell_edit(0, Handle::NULL, cell_center(0, 0)),
        TableCellEditStart::Started
    );
    assert!(app.table_cell_edit.is_some());
    let _ = app.update(super::Message::CommandSubmit);
    assert!(app.tabs[0].active_cmd.is_none(), "Enter ends the editor");
    assert!(app.table_cell_edit.is_none(), "no cell stays highlighted");
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

/// Type `text` into cell (`row`, `column`) through the real editor flow.
fn type_into_cell(app: &mut OpenCADStudio, row: usize, column: usize, text: &str) {
    assert_eq!(
        app.begin_table_cell_edit(0, Handle::NULL, cell_center(row, column)),
        TableCellEditStart::Started
    );
    let _ = app.update(super::Message::CommandInput(text.to_string()));
    let _ = app.update(super::Message::CommandSubmit);
    assert!(app.tabs[0].active_cmd.is_none(), "Enter ends the editor");
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
