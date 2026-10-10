//! THIRD_PARTY_LICENSES.txt: every crate compiled into the shipped binaries (`cargo tree` of
//! their normal dependencies for the platform being packaged), with its licence and the licence
//! texts it ships. Identical texts are printed once, followed by the crates they cover. Crates
//! published without the text of their licence point to the standard texts in
//! `rust/packaging/licenses` (with their copyright holders from Cargo.toml); a crate covered by
//! neither stops the release.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::Result;
use crate::cargo::{self, Metadata};

/// Standard texts shipped in `licenses/`: (SPDX id, phrase found in that text, file name).
pub const STANDARD: [(&str, &str, &str); 3] = [
    ("Apache-2.0", "Apache License", "Apache-2.0.txt"),
    ("BSL-1.0", "Boost Software License", "BSL-1.0.txt"),
    ("MIT", "Permission is hereby granted", "MIT.txt"),
];

/// A crate in the binaries and the licence texts found in its source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Crate {
    pub name: String,
    pub version: String,
    pub license: String,
    pub authors: Vec<String>,
    pub repository: String,
    /// (file name, text)
    pub texts: Vec<(String, String)>,
}

/// The licence ids of an SPDX expression ("(MIT OR Apache-2.0) AND OFL-1.1", "MIT/Apache-2.0").
pub fn spdx_ids(expression: &str) -> Vec<&str> {
    expression
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '/'))
        .filter(|w| !w.is_empty() && !matches!(*w, "OR" | "AND" | "WITH"))
        .collect()
}

impl Crate {
    fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }

    fn has(&self, id: &str) -> bool {
        spdx_ids(&self.license).contains(&id)
    }

    /// The standard texts that apply: every known licence of a crate shipping no text at all,
    /// and, for a crate under several licences at once (`AND`), each one whose text is missing.
    pub fn standard_texts(&self) -> Vec<&'static str> {
        let all_needed = self.license.contains(" AND ");
        if !self.texts.is_empty() && !all_needed {
            return Vec::new();
        }
        STANDARD
            .iter()
            .filter(|(id, phrase, _)| {
                self.has(id) && !self.texts.iter().any(|(_, t)| t.contains(phrase))
            })
            .map(|(_, _, file)| *file)
            .collect()
    }

    /// Neither a licence text of its own nor a standard one: the release must not ship it.
    pub fn uncovered(&self) -> bool {
        self.texts.is_empty() && self.standard_texts().is_empty()
    }
}

fn is_license_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "LICENSE",
        "LICENCE",
        "COPYING",
        "NOTICE",
        "UNLICENSE",
        "COPYRIGHT",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    let trimmed = text.trim_end().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// The licence files in a crate's folder (and its `LICENSES/` folder), sorted by name.
pub fn license_texts(dir: &Path, license_file: Option<&str>) -> Vec<(String, String)> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let mut add_dir = |folder: &Path, prefix: &str, all: bool| {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if path.is_file() && (all || is_license_name(&name)) {
                if let Some(text) = read_text(&path) {
                    found.insert(format!("{prefix}{name}"), text);
                }
            }
        }
    };
    add_dir(dir, "", false);
    add_dir(&dir.join("LICENSES"), "LICENSES/", true);
    // Bundled fonts keep their licences next to them (epaint_default_fonts: OFL, UFL, Hack).
    if let Ok(fonts) = std::fs::read_dir(dir.join("fonts")) {
        for entry in fonts.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".txt") {
                if let Some(text) = read_text(&entry.path()) {
                    found.insert(format!("fonts/{name}"), text);
                }
            }
        }
    }
    if let Some(file) = license_file {
        if let Some(text) = read_text(&dir.join(file)) {
            found.insert(file.replace('\\', "/"), text);
        }
    }
    found.into_iter().collect()
}

/// "Name <mail>" → "Name".
fn author_name(author: &str) -> String {
    author.split('<').next().unwrap_or(author).trim().to_owned()
}

/// The crates named in `shipped` (name, version), workspace members left out, sorted; every
/// one of them must be found in the metadata.
pub fn crates(metadata: &Metadata, shipped: &[(String, String)]) -> Result<Vec<Crate>> {
    let members: BTreeSet<(&str, &str)> = metadata
        .packages()
        .iter()
        .filter(|p| p["id"].as_str().is_some_and(|id| metadata.is_member(id)))
        .map(|p| {
            (
                p["name"].as_str().unwrap_or_default(),
                p["version"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let wanted: BTreeSet<(&str, &str)> = shipped
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .filter(|key| !members.contains(key))
        .collect();
    let mut out: Vec<Crate> = metadata
        .packages()
        .iter()
        .filter(|p| !p["id"].as_str().is_some_and(|id| metadata.is_member(id)))
        .filter(|p| {
            let key = (
                p["name"].as_str().unwrap_or_default(),
                p["version"].as_str().unwrap_or_default(),
            );
            wanted.contains(&key)
        })
        .map(|p| {
            let text = |key: &str| p[key].as_str().unwrap_or_default().to_owned();
            let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default())
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            Crate {
                name: text("name"),
                version: text("version"),
                license: p["license"].as_str().unwrap_or("（未標示）").to_owned(),
                authors: p["authors"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .map(author_name)
                            .collect()
                    })
                    .unwrap_or_default(),
                repository: text("repository"),
                texts: license_texts(&dir, p["license_file"].as_str()),
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    out.dedup_by(|a, b| a.name == b.name && a.version == b.version);
    if out.len() != wanted.len() {
        let found: BTreeSet<(&str, &str)> = out
            .iter()
            .map(|c| (c.name.as_str(), c.version.as_str()))
            .collect();
        let missing: Vec<String> = wanted
            .difference(&found)
            .map(|(n, v)| format!("{n} {v}"))
            .collect();
        return Err(format!(
            "cargo tree 列出的套件在 cargo metadata 中找不到：{}",
            missing.join("、")
        ));
    }
    Ok(out)
}

/// The crates of the shipped binaries for `triple`, refusing any without a licence text.
pub fn collect(workspace: &Path, triple: &str, shipped: &[&str]) -> Result<Vec<Crate>> {
    let metadata = Metadata::load(workspace, triple)?;
    let tree = cargo::tree(workspace, triple, shipped)?;
    let crates = crates(&metadata, &tree)?;
    if crates.is_empty() {
        return Err(format!(
            "{triple}：找不到任何相依套件（cargo tree 的結果不對）"
        ));
    }
    let uncovered: Vec<String> = crates
        .iter()
        .filter(|c| c.uncovered())
        .map(|c| format!("{} {}（{}）", c.name, c.version, c.license))
        .collect();
    if !uncovered.is_empty() {
        return Err(format!(
            "{triple}：這些套件沒有授權全文，請把標準全文加到 rust/packaging/licenses 並列入 \
             licenses::STANDARD：{}",
            uncovered.join("、")
        ));
    }
    Ok(crates)
}

fn standard_line(c: &Crate, files: &[&str]) -> String {
    let holders = if c.authors.is_empty() {
        String::new()
    } else {
        format!("；著作權人：{}", c.authors.join("、"))
    };
    format!(
        "{}（{}{holders}）：{}",
        c.label(),
        c.license,
        files.join("、")
    )
}

/// The whole file.
pub fn render(product: &str, version: &str, crates: &[Crate]) -> String {
    let mut out = String::new();
    let rule = "=".repeat(78);
    let _ = writeln!(
        out,
        "{product} {version} — 第三方元件授權 / Third-party licences"
    );
    let _ = writeln!(out, "{rule}");
    let _ = writeln!(
        out,
        "執行檔靜態連結了下列 {} 個 Rust 套件（crates.io）。授權全文在清單之後；\n\
         相同的全文只列一次，並註明適用的套件。\n",
        crates.len()
    );
    for c in crates {
        let _ = writeln!(out, "{:<40} {:<36} {}", c.label(), c.license, c.repository);
    }
    let mut groups: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
    let mut standard: Vec<String> = Vec::new();
    let mut mpl: Vec<String> = Vec::new();
    for c in crates {
        let files = c.standard_texts();
        if !files.is_empty() {
            standard.push(standard_line(c, &files));
        }
        if c.has("MPL-2.0") {
            mpl.push(format!(
                "{}：https://crates.io/crates/{}/{}",
                c.label(),
                c.name,
                c.version
            ));
        }
        for (name, text) in &c.texts {
            groups
                .entry((text.as_str(), name.as_str()))
                .or_default()
                .push(c.label());
        }
    }
    // One text under several file names (LICENSE-MIT, LICENSE): keep the first name.
    let mut texts: BTreeMap<&str, (&str, BTreeSet<String>)> = BTreeMap::new();
    for ((text, name), users) in groups {
        let entry = texts.entry(text).or_insert((name, BTreeSet::new()));
        entry.1.extend(users);
    }
    let mut ordered: Vec<(&str, &str, BTreeSet<String>)> = texts
        .into_iter()
        .map(|(text, (name, users))| (text, name, users))
        .collect();
    ordered.sort_by(|a, b| a.2.iter().next().cmp(&b.2.iter().next()).then(a.1.cmp(b.1)));
    for (text, name, users) in ordered {
        let list: Vec<String> = users.into_iter().collect();
        let _ = writeln!(
            out,
            "\n{rule}\n{name} — 適用於：{}\n{rule}\n{text}",
            list.join("、")
        );
    }
    if !standard.is_empty() {
        let _ = writeln!(
            out,
            "\n{rule}\n下列套件的原始碼沒有附該授權的全文；依其授權標示，適用本資料夾的標準全文\n\
             （以 MIT OR Apache-2.0 雙授權的套件，即依其中任一授權散布）：\n{rule}\n{}",
            standard.join("\n")
        );
    }
    if !mpl.is_empty() {
        let _ = writeln!(
            out,
            "\n{rule}\n以 MPL-2.0 授權的套件未經修改，原始碼可在此取得：\n{rule}\n{}",
            mpl.join("\n")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(name: &str, license: &str, texts: &[(&str, &str)]) -> Crate {
        Crate {
            name: name.into(),
            version: "1.0.0".into(),
            license: license.into(),
            texts: texts
                .iter()
                .map(|(n, t)| ((*n).to_owned(), (*t).to_owned()))
                .collect(),
            ..Crate::default()
        }
    }

    #[test]
    fn spdx_expressions_are_split_into_ids() {
        assert_eq!(spdx_ids("MIT OR Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(
            spdx_ids("(MIT OR Apache-2.0) AND OFL-1.1"),
            ["MIT", "Apache-2.0", "OFL-1.1"]
        );
        assert_eq!(spdx_ids("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(
            spdx_ids("Apache-2.0 WITH LLVM-exception"),
            ["Apache-2.0", "LLVM-exception"]
        );
        assert!(!krate("x", "MIT-0", &[]).has("MIT"));
    }

    #[test]
    fn identical_texts_are_printed_once() {
        let crates = [
            krate("a", "MIT", &[("LICENSE-MIT", "MIT text")]),
            krate("b", "MIT", &[("LICENSE", "MIT text")]),
            krate("c", "ISC", &[("COPYING", "other")]),
            krate("d", "ISC", &[]),
        ];
        let text = render("OpenDRS", "0.1.0", &crates);
        assert_eq!(text.matches("MIT text").count(), 1);
        assert!(text.contains("適用於：a 1.0.0、b 1.0.0"));
        assert!(text.contains("COPYING — 適用於：c 1.0.0"));
        assert!(
            crates[3].uncovered(),
            "ISC without a text cannot be shipped"
        );
    }

    #[test]
    fn crates_without_texts_point_to_the_standard_ones() {
        let dual = krate("egui", "MIT OR Apache-2.0", &[]);
        assert_eq!(dual.standard_texts(), ["Apache-2.0.txt", "MIT.txt"]);
        assert!(!dual.uncovered());
        let objc = Crate {
            authors: vec!["Mads Marquart".into()],
            ..krate("objc2", "MIT", &[])
        };
        assert_eq!(objc.standard_texts(), ["MIT.txt"]);
        let boost = krate("clipboard-win", "BSL-1.0", &[]);
        assert_eq!(boost.standard_texts(), ["BSL-1.0.txt"]);
        let shipped = krate(
            "x",
            "MIT OR Apache-2.0",
            &[("LICENSE-APACHE", "Apache License")],
        );
        assert!(
            shipped.standard_texts().is_empty(),
            "one of the two is enough"
        );
        let fonts = krate(
            "epaint_default_fonts",
            "(MIT OR Apache-2.0) AND OFL-1.1",
            &[
                ("fonts/OFL.txt", "SIL Open Font License"),
                ("fonts/m.txt", "Permission is hereby granted"),
            ],
        );
        assert_eq!(fonts.standard_texts(), ["Apache-2.0.txt"]);
        let text = render("OpenDRS", "0.1.0", &[dual, objc, boost]);
        assert!(text.contains("egui 1.0.0（MIT OR Apache-2.0）：Apache-2.0.txt、MIT.txt"));
        assert!(text.contains("objc2 1.0.0（MIT；著作權人：Mads Marquart）：MIT.txt"));
        assert!(text.contains("clipboard-win 1.0.0（BSL-1.0）：BSL-1.0.txt"));
    }

    #[test]
    fn mpl_crates_say_where_their_source_is() {
        let serial = krate(
            "serialport",
            "MPL-2.0",
            &[("LICENSE.txt", "Mozilla Public License")],
        );
        let text = render("OpenDRS", "0.1.0", &[serial]);
        assert!(text.contains("serialport 1.0.0：https://crates.io/crates/serialport/1.0.0"));
    }

    #[test]
    fn authors_lose_their_mail_address() {
        assert_eq!(author_name("Ann Lee <ann@example.com>"), "Ann Lee");
        assert_eq!(
            author_name("The Rust Project Developers"),
            "The Rust Project Developers"
        );
    }

    #[test]
    fn licence_files_are_recognised() {
        for name in [
            "LICENSE",
            "license-mit",
            "LICENCE.txt",
            "COPYING",
            "NOTICE",
            "UNLICENSE",
        ] {
            assert!(is_license_name(name), "{name}");
        }
        for name in ["README.md", "Cargo.toml", "src"] {
            assert!(!is_license_name(name), "{name}");
        }
    }
}
