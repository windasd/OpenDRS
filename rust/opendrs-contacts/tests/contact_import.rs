//! Port of tests/test_contact_import.py, plus the contact-list growth checks of
//! test_codeplug.py and test_radio_link.py that use `append_contacts` (simulated radio only).

mod common;

use std::sync::Arc;

use common::{SAMPLES, SMALL, fixtures, load, user};
use opendrs_contacts::contact_import::{
    DEFAULT_CONTACT_CAPACITY, ImportReport, NameFormat, NewContact, append_contacts, contact_text,
    import_users, is_radioid_style_name,
};
use opendrs_contacts::radioid::RadioIdUser;
use opendrs_core::codeplug::{self, BLOCK, Counts};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_link::RadioClient;
use opendrs_core::radio_sim::SimulatedRadio;
use opendrs_core::records::Contact;
use opendrs_core::textcodec::{NameCodec, display_name, fit_name};

const BIG5: NameCodec = NameCodec::Big5;
const ASCII: NameCodec = NameCodec::Ascii;

fn import(
    project: &Project,
    users: &[RadioIdUser],
    fmt: NameFormat,
    codec: NameCodec,
) -> (Project, ImportReport) {
    import_users(project, users, fmt, codec, DEFAULT_CONTACT_CAPACITY).unwrap()
}

fn append(project: &Project, entries: &[NewContact], codec: NameCodec) -> (Project, ImportReport) {
    append_contacts(project, entries, codec, DEFAULT_CONTACT_CAPACITY).unwrap()
}

#[test]
fn name_formats() {
    let u = user(9990001, "QA0BD", "James", "");
    for (fmt, expected) in [
        (NameFormat::Callsign, "QA0BD"),
        (NameFormat::CallsignName, "QA0BD James"),
        (NameFormat::Name, "James"),
    ] {
        assert_eq!(contact_text(&u, fmt), expected);
    }
}

#[test]
fn name_format_fallbacks() {
    assert_eq!(
        contact_text(&user(9990001, "", "", ""), NameFormat::Callsign),
        "9990001"
    );
    assert_eq!(
        contact_text(&user(9990001, "QA0BD", "", ""), NameFormat::Name),
        "QA0BD"
    );
    assert!(NameFormat::ALL.iter().all(|f| !f.label().is_empty()));
    assert_eq!(NameFormat::ALL.len(), 3);
    assert_eq!(
        NameFormat::from_value("callsign_name"),
        Some(NameFormat::CallsignName)
    );
}

#[test]
fn fit_name_cases() {
    let cases = [
        ("QA0BD James", ASCII, "QA0BD Jame"), // cut at 10 bytes
        ("José Núñez", ASCII, "Jose Nunez"),  // accents folded
        ("王小明測試字", BIG5, "王小明測試"), // 5 CJK = 10 bytes
        ("A王小明測試", BIG5, "A王小明測"),   // never split a CJK char
        ("QA0BD\r\n", ASCII, "QA0BD"),        // legacy CR/LF bug never written
        ("ab😀cd", BIG5, "abcd"),             // unrepresentable dropped
        ("ab        x", ASCII, "ab"),         // trailing space trimmed
    ];
    for (text, codec, stored) in cases {
        let (field, got) = fit_name(text, 10, codec);
        assert_eq!(got, stored);
        assert_eq!(field.len(), 10);
        let end = field.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        assert_eq!(codec.decode(&field[..end]).unwrap(), stored);
    }
}

#[test]
fn display_shows_legacy_control_characters() {
    assert_eq!(display_name(b"QA0BD\r\n\0\0\0", ASCII), "QA0BD␍␊");
}

#[test]
fn import_into_real_project() {
    let project = load(SMALL);
    let before = project.contacts.len();
    let users = [
        user(9990001, "QA0BD", "James", ""),
        user(9990002, "QA0ADN", "Chen", ""),
    ];
    let (updated, report) = import(&project, &users, NameFormat::Callsign, BIG5);
    assert_eq!(
        report,
        ImportReport {
            added: 2,
            ..ImportReport::default()
        }
    );
    let added = &updated.contacts[before..];
    let indices: Vec<usize> = added.iter().map(|c| usize::from(c.index())).collect();
    assert_eq!(indices, [before + 1, before + 2]);
    assert!(added.iter().all(|c| c.kind() == Contact::PRIVATE_CALL));
    let named: Vec<(String, u32)> = added.iter().map(|c| (c.name(BIG5), c.call_id())).collect();
    assert_eq!(
        named,
        [
            ("QA0BD".to_string(), 9990001),
            ("QA0ADN".to_string(), 9990002)
        ]
    );
    let reloaded = f136::parse(&f136::serialize(&updated)).unwrap();
    assert_eq!(reloaded, updated);
    // image: counts at 0x138F and the appended contact copy change, nothing else
    let (img_before, img_after) = (&project.image, &updated.image);
    let changed = (0..img_before.len()).filter(|&i| img_before[i] != img_after[i]);
    assert!(
        changed
            .into_iter()
            .all(|i| (0x138F..0x1395).contains(&i) || i >= 0x1400)
    );
    let count = |at: usize| usize::from(u16::from_le_bytes([img_after[at], img_after[at + 1]]));
    assert_eq!(
        (count(0x138F), count(0x1391), count(0x1393)),
        (
            updated.zones.len(),
            updated.channels.len(),
            updated.contacts.len()
        )
    );
    let raw: Vec<u8> = updated
        .contacts
        .iter()
        .flat_map(|c| c.raw().to_vec())
        .collect();
    assert_eq!(&img_after[0x1400..0x1400 + raw.len()], &raw[..]);
}

#[test]
fn existing_ids_are_overwritten_not_duplicated() {
    for sample in SAMPLES {
        let project = load(sample);
        let existing: Vec<u32> = project
            .contacts
            .iter()
            .filter(|c| c.kind() == Contact::PRIVATE_CALL)
            .map(Contact::call_id)
            .collect();
        let mut users: Vec<RadioIdUser> = existing
            .iter()
            .map(|&i| user(i, &format!("X{}", i % 100_000), "", ""))
            .collect();
        users.push(user(9999999, "OLD", "", ""));
        users.push(user(9999999, "NEW", "", ""));
        let (updated, report) = import(&project, &users, NameFormat::Callsign, BIG5);
        assert_eq!(
            (report.added, report.updated, report.unchanged),
            (1, existing.len(), 0),
            "{sample}"
        );
        assert_eq!(updated.contacts.len(), project.contacts.len() + 1);
        for (old, new) in project.contacts.iter().zip(&updated.contacts) {
            assert_eq!(
                (new.index(), new.kind(), new.call_id()),
                (old.index(), old.kind(), old.call_id())
            );
            assert_eq!(
                (&new.raw()[..3], &new.raw()[13..]),
                (&old.raw()[..3], &old.raw()[13..])
            );
            if old.kind() == Contact::PRIVATE_CALL {
                assert_eq!(new.name(BIG5), format!("X{}", old.call_id() % 100_000));
            } else {
                assert_eq!(new.raw(), old.raw());
            }
        }
        assert_eq!(updated.contacts.last().unwrap().name(BIG5), "NEW"); // later duplicate wins
        let raw: Vec<u8> = updated
            .contacts
            .iter()
            .flat_map(|c| c.raw().to_vec())
            .collect();
        assert_eq!(&updated.image[0x1400..0x1400 + raw.len()], &raw[..]);
    }
}

#[test]
fn same_name_counts_as_unchanged() {
    let users = [user(9991234, "QA0BA", "", "")];
    let (project, _) = import(&load(SMALL), &users, NameFormat::Callsign, ASCII);
    let (again, report) = import(&project, &users, NameFormat::Callsign, ASCII);
    assert_eq!((report.added, report.updated, report.unchanged), (0, 0, 1));
    assert_eq!(again.contacts, project.contacts);
}

#[test]
fn capacity_limit() {
    let project = load(SMALL);
    let room = 3;
    let users: Vec<RadioIdUser> = (0..5)
        .map(|i| user(9991000 + i, &format!("C{i}"), "", ""))
        .collect();
    let capacity = project.contacts.len() + room;
    let (updated, report) =
        import_users(&project, &users, NameFormat::Callsign, ASCII, capacity).unwrap();
    assert_eq!((report.added, report.over_capacity), (room, 2));
    assert_eq!(updated.contacts.len(), project.contacts.len() + room);
    assert!(report.summary().contains("超過容量"));
}

#[test]
fn report_lists_shortened_names() {
    let users = [user(9990001, "QA0BD", "Jonathan", "")];
    let (_, report) = import(&Project::default(), &users, NameFormat::CallsignName, ASCII);
    assert_eq!(
        report.shortened,
        [(
            9990001,
            "QA0BD Jonathan".to_string(),
            "QA0BD Jona".to_string()
        )]
    );
    assert!(report.summary().contains("截短"));
}

#[test]
fn import_into_empty_project_creates_section() {
    let users = [user(9990001, "QA0BD", "", "")];
    let (updated, report) = import(&Project::default(), &users, NameFormat::Callsign, ASCII);
    assert_eq!(report.added, 1);
    assert_eq!(updated.section_order, ["Contacts"]);
    assert_eq!(updated.contacts[0].index(), 1);
    assert!(f136::serialize(&updated).starts_with(b"Contacts\t1\t0\t5\t81\t65")); // "QA"
}

#[test]
fn contact_build_validation() {
    let field = [0u8; 10];
    assert!(Contact::build(1, 5, b"short", 1).is_err());
    assert!(Contact::build(1, 5, &field, 0).is_err());
    assert_eq!(
        Contact::build(1, 7, &field, 16777184).unwrap().kind_label(),
        "無地址呼"
    );
}

#[test]
fn appending_past_index_65535_fails_like_python() {
    let full = Project {
        contacts: vec![Contact::build(65535, 5, &[0; 10], 1).unwrap()],
        ..Project::default()
    };
    let err = append_contacts(&full, &[NewContact::new(5, "X", 2)], ASCII, 1000).unwrap_err();
    assert_eq!(err.to_string(), "索引或類型超出範圍");
}

#[test]
fn radioid_style_names() {
    let cases = [
        ("QA0BD", true), // outdated callsign: DMR ID kept, callsign changed
        ("QA0BD James", true),
        ("QA0BD␍␊James", true), // legacy name with CR/LF
        ("James", true),        // matches RadioID first name
        ("Christ", true),       // truncated first name
        ("9990001", true),      // bare ID
        ("", true),
        ("QA0AKL/P", true),
        ("老王", false),
        ("台中中繼", false),
        ("Dad", false),
        ("TG46001", false),
    ];
    for (existing, replace) in cases {
        let first = if existing == "Christ" {
            "Christopher"
        } else {
            "James"
        };
        let u = RadioIdUser {
            first_name: first.into(),
            ..user(9990001, "QA0AGX", "", "")
        };
        assert_eq!(is_radioid_style_name(existing, &u), replace, "{existing}");
    }
}

#[test]
fn custom_names_are_kept_and_callsigns_updated() {
    let (base, _) = append(
        &Project::default(),
        &[
            NewContact::new(Contact::PRIVATE_CALL, "QA0BD", 9990001),
            NewContact::new(Contact::PRIVATE_CALL, "老王", 9990002),
        ],
        BIG5,
    );
    let users = [
        user(9990001, "QA0AGX", "", ""),
        user(9990002, "QA0AGQ", "", ""),
        user(9990003, "QA0AGP", "", ""),
    ];
    let (updated, report) = import(&base, &users, NameFormat::Callsign, BIG5);
    let names: Vec<String> = updated.contacts.iter().map(|c| c.name(BIG5)).collect();
    assert_eq!(names, ["QA0AGX", "老王", "QA0AGP"]);
    assert_eq!(
        (report.added, report.updated, report.unchanged, report.kept),
        (1, 1, 0, 1)
    );
    assert!(report.summary().contains("保留原名稱"));
    assert_eq!(updated.contacts[1].raw(), base.contacts[1].raw());
}

#[test]
fn replace_if_is_ignored_by_equality() {
    let plain = NewContact::new(5, "A", 1);
    let guarded = plain.clone().with_replace_if(Arc::new(|_| false));
    assert_eq!(plain, guarded);
}

#[test]
fn more_contacts_grow_part1_and_keep_everything_consistent() {
    // test_codeplug.py
    let written = f136::load(fixtures().join("capture/written_project.F136")).unwrap();
    let extra: Vec<NewContact> = (0..369)
        .map(|i| NewContact::new(Contact::PRIVATE_CALL, format!("C{i}"), 4670000 + i))
        .collect();
    let (big, report) = append(&written, &extra, ASCII);
    assert_eq!((report.added, big.contacts.len()), (369, 729));
    let stream = codeplug::build_stream(&big, None).unwrap();
    let counts = Counts::of(&big);
    assert_eq!(counts.part1_blocks(), 9);
    assert_eq!(stream.len(), counts.stream_size());
    assert_eq!(
        codeplug::parse_stream(&stream, counts).unwrap().contacts,
        big.contacts
    );
    assert_eq!(&stream[9 * BLOCK..9 * BLOCK + 32], big.zones[0].raw()); // zones follow part 1
}

#[test]
fn simulated_write_grows_part1_for_more_contacts() {
    // test_radio_link.py, against the in-memory simulated radio (no serial port).
    let stream = std::fs::read(fixtures().join("capture/read_stream.bin")).unwrap();
    let mut radio = SimulatedRadio::with_stream(&stream, Counts::new(4, 27, 360));
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    let extra: Vec<NewContact> = (0..369)
        .map(|i| NewContact::new(Contact::PRIVATE_CALL, format!("BX{i}"), 4670000 + i))
        .collect();
    let (big, _) = append(&project, &extra, ASCII);
    RadioClient::new(&mut radio)
        .write_codeplug(&big, None, None)
        .unwrap();
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(back.counts, Counts::new(4, 27, 729));
    assert_eq!(back.counts.part1_blocks(), 9);
    assert_eq!(back.project.contacts, big.contacts);
    assert_eq!(
        (&back.project.zones, &back.project.channels),
        (&big.zones, &big.channels)
    );
}
