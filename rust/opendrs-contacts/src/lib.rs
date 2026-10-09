//! OpenDRS contacts: the RadioID.net user database, importing its users as contacts, and the
//! contact list as CSV — a Rust port of the Python modules `radioid.py`, `contact_import.py` and
//! `contacts_csv.py` (src/opendrs), kept byte-for-byte compatible with them.
//!
//! Text is split, folded, validated and quoted with `opendrs_core::pystr` and
//! `opendrs_core::pycsv` (Python's `str` methods and `csv` module), exactly as the Python
//! version does.

pub mod contact_import;
pub mod contacts_csv;
pub mod radioid;

pub use opendrs_core::{pycsv, pystr};

pub use contact_import::{ImportReport, NameFormat, NewContact, append_contacts, import_users};
pub use contacts_csv::{export_contacts, parse_contacts_csv};
pub use radioid::{Query, RadioIdError, RadioIdUser, UserDb};
