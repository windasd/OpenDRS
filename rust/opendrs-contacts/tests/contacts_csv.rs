//! Port of tests/test_contacts_csv.py, plus the CSV import step of test_gui_models.py.

mod common;

use common::{SAMPLES, SMALL, load};
use opendrs_contacts::contact_import::{
    DEFAULT_CONTACT_CAPACITY, ImportReport, NewContact, append_contacts,
};
use opendrs_contacts::contacts_csv::{HEADER, export_contacts, parse_contacts_csv};
use opendrs_core::f136::Project;
use opendrs_core::records::Contact;
use opendrs_core::textcodec::NameCodec;

const BIG5: NameCodec = NameCodec::Big5;

fn append(project: &Project, entries: &[NewContact], codec: NameCodec) -> (Project, ImportReport) {
    append_contacts(project, entries, codec, DEFAULT_CONTACT_CAPACITY).unwrap()
}

#[test]
fn export_is_excel_friendly_utf8() {
    let project = load(SMALL);
    let data = export_contacts(&project, BIG5);
    assert!(data.starts_with(b"\xef\xbb\xbf"));
    let text = std::str::from_utf8(&data[3..]).unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(lines[0], HEADER.join(","));
    assert_eq!(lines[1], "1,組呼,TG46001_CN,46001");
    assert_eq!(
        lines.iter().filter(|l| !l.is_empty()).count(),
        project.contacts.len() + 1
    );
}

#[test]
fn export_then_import_round_trip() {
    for sample in SAMPLES {
        let project = load(sample);
        let result = parse_contacts_csv(&export_contacts(&project, BIG5)).unwrap();
        assert!(result.errors.is_empty(), "{sample}: {:?}", result.errors);
        let parsed: Vec<(u8, u32)> = result.entries.iter().map(|e| (e.kind, e.call_id)).collect();
        let original: Vec<(u8, u32)> = project
            .contacts
            .iter()
            .map(|c| (c.kind(), c.call_id()))
            .collect();
        assert_eq!(parsed, original);
        let (_, report) = append(&project, &result.entries, BIG5);
        assert_eq!(report.added, 0);
        assert_eq!(report.updated + report.unchanged, project.contacts.len());
        assert!(report.unchanged > 0);
    }
}

#[test]
fn import_chinese_headers_and_legacy_encodings() {
    let text = "編號,類型,名稱,呼叫ID\r\n1,個呼,王小明,9991234\r\n2,組呼,台灣,46600\r\n,,,\r\n";
    let utf8 = text.as_bytes().to_vec();
    let with_bom = [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat();
    let big5 = BIG5.encode(text).unwrap();
    for data in [utf8, with_bom, big5] {
        let result = parse_contacts_csv(&data).unwrap();
        assert!(result.errors.is_empty());
        assert_eq!(
            result.entries,
            [
                NewContact::new(5, "王小明", 9991234),
                NewContact::new(4, "台灣", 46600)
            ]
        );
    }
}

#[test]
fn import_gbk_and_minimal_columns() {
    let data = NameCodec::Gbk
        .encode("名称,号码\r\n张伟,4600001\r\n")
        .unwrap();
    let result = parse_contacts_csv(&data).unwrap();
    assert_eq!(result.encoding, "gbk");
    assert_eq!(result.entries, [NewContact::new(5, "张伟", 4600001)]); // type defaults to private
}

#[test]
fn import_unusable_files() {
    for (data, error) in [(b"".as_slice(), "空"), (b"Foo,Bar\r\n1,2\r\n", "缺少欄位")] {
        let result = parse_contacts_csv(data).unwrap();
        assert!(result.entries.is_empty());
        assert!(result.errors[0].contains(error), "{:?}", result.errors);
    }
}

#[test]
fn import_reports_bad_rows_and_keeps_good_ones() {
    let data = b"Name,ID,CallType\r\n\
                 OK,9990001,private\r\n\
                 BadId,abc,\r\n\
                 TooBig,99999999,\r\n\
                 BadType,9990002,weird\r\n\
                 ,9990003,All Call\r\n";
    let result = parse_contacts_csv(data).unwrap();
    let texts: Vec<&str> = result.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["OK", "9990003"]);
    assert_eq!(result.entries[1].kind, 6);
    assert_eq!(result.errors.len(), 3);
    assert!(result.errors.iter().any(|e| e.contains("第 3 列")));
}

#[test]
fn append_keeps_existing_indices_and_mixed_types() {
    let project = load(SMALL);
    let n = project.contacts.len();
    let entries = [
        NewContact::new(4, "TG99", 99),
        NewContact::new(5, "QA0BD", 9990001),
    ];
    let (updated, report) = append(&project, &entries, BIG5);
    assert_eq!(updated.contacts[..n], project.contacts[..]);
    let tail: Vec<(usize, u8)> = updated.contacts[n..]
        .iter()
        .map(|c| (usize::from(c.index()), c.kind()))
        .collect();
    assert_eq!(tail, [(n + 1, 4), (n + 2, 5)]);
    assert_eq!(report.added, 2);
}

#[test]
fn same_id_different_type_is_not_a_duplicate() {
    let ascii = NameCodec::Ascii;
    let (base, _) = append(
        &Project::default(),
        &[NewContact::new(4, "TG", 9990001)],
        ascii,
    );
    let (updated, report) = append(&base, &[NewContact::new(5, "P", 9990001)], ascii);
    assert_eq!(report.added, 1);
    let kinds: Vec<u8> = updated.contacts.iter().map(Contact::kind).collect();
    assert_eq!(kinds, [Contact::GROUP_CALL, Contact::PRIVATE_CALL]);
}

#[test]
fn big5_file_with_a_bad_row_imports_the_good_one() {
    // The CSV step of test_gui_models.py::test_csv_export_import_and_recent_menu.
    let data = BIG5
        .encode("名稱,ID\r\n測試甲,9991111\r\n壞,xx\r\n")
        .unwrap();
    let result = parse_contacts_csv(&data).unwrap();
    assert_eq!(result.encoding, "big5");
    assert_eq!(result.errors, ["第 3 列：ID「xx」不是數字"]);
    let project = load(SMALL);
    let (updated, report) = append(&project, &result.entries, BIG5);
    assert_eq!(report.added, 1);
    assert_eq!(updated.contacts.last().unwrap().name(BIG5), "測試甲");
}

#[test]
fn oversized_field_is_a_csv_error() {
    let mut data = b"Name,ID\r\n".to_vec();
    data.extend(std::iter::repeat_n(b'x', 131_073));
    data.extend(b",1\r\n");
    let err = parse_contacts_csv(&data).unwrap_err();
    assert_eq!(err.to_string(), "field larger than field limit (131072)");
}
