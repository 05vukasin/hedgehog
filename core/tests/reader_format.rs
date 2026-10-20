//! The reader picks the format from the file's content, not its extension; delimited text is
//! recognized by a text extension or by content that is not a workbook container.

use hh_core::{open, Value};

fn xlsx_bytes() -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet().set_name("Data").unwrap();
    ws.write_string(0, 0, "Grad").unwrap();
    ws.write_number(1, 0, 42.0).unwrap();
    wb.save_to_buffer().unwrap()
}

#[test]
fn xlsx_with_wrong_extension_opens() {
    let bytes = xlsx_bytes();
    for ext in ["xls", "ods", "xlsb", "bin"] {
        let path = std::env::temp_dir().join(format!("hh_test_{}_misnamed.{ext}", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let wb = open(&path);
        std::fs::remove_file(&path).ok();
        let wb = wb.unwrap_or_else(|e| panic!(".{ext}: {e}"));
        assert_eq!(wb.sheets[0].name, "Data");
        assert_eq!(wb.sheets[0].cells[1].value, Some(Value::Number(42.0)));
    }
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    let path = std::env::temp_dir().join(format!("hh_test_{}_garbage.xls", std::process::id()));
    std::fs::write(&path, b"PK\x03\x04 not really a zip").unwrap();
    let r = open(&path);
    std::fs::remove_file(&path).ok();
    assert!(r.is_err());
}

#[test]
fn delimited_text_by_extension_or_content() {
    let csv = b"Grad;Iznos\nNis;42\n";
    for ext in ["csv", "CSV", "txt", "dat", "asp"] {
        let path = std::env::temp_dir().join(format!("hh_test_{}_text.{ext}", std::process::id()));
        std::fs::write(&path, csv).unwrap();
        let wb = open(&path);
        std::fs::remove_file(&path).ok();
        let wb = wb.unwrap_or_else(|e| panic!(".{ext}: {e}"));
        assert_eq!(wb.sheets.len(), 1);
        assert_eq!(wb.sheets[0].name, format!("hh_test_{}_text", std::process::id()));
        assert_eq!(wb.sheets[0].cells[3].value, Some(Value::Number(42.0)));
    }
    // A workbook saved as .csv opens as the workbook; text named .xls stays an error.
    let path = std::env::temp_dir().join(format!("hh_test_{}_book.csv", std::process::id()));
    std::fs::write(&path, xlsx_bytes()).unwrap();
    let wb = open(&path);
    std::fs::remove_file(&path).ok();
    assert_eq!(wb.unwrap().sheets[0].name, "Data");
    let path = std::env::temp_dir().join(format!("hh_test_{}_text.xls", std::process::id()));
    std::fs::write(&path, csv).unwrap();
    let r = open(&path);
    std::fs::remove_file(&path).ok();
    assert!(r.is_err());
}
