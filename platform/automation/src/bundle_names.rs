// Copyright (c) 2026 Cua AI, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Derived from cua-driver (https://github.com/trycua/cua):
// libs/cua-driver/rust/crates/platform-macos/src/apps/names.rs and
// libs/cua-driver/rust/crates/platform-macos/src/apps/mod.rs.

//! Finding an installed `.app` bundle by any name it ships: its file name,
//! `CFBundleDisplayName` / `CFBundleName` and their localizations.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSData, NSDictionary, NSHomeDirectory, NSLocale, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString,
};

use crate::app_names::{self, AppName, NameMiss, NamedApp};

/// The metadata keys that hold a bundle's shown name, display name first.
const NAME_KEYS: [&str; 2] = ["CFBundleDisplayName", "CFBundleName"];

/// The language each pre-ISO `.lproj` folder name stands for.
const LEGACY_LANGUAGES: [(&str, &str); 19] = [
    ("English", "en"),
    ("French", "fr"),
    ("German", "de"),
    ("Japanese", "ja"),
    ("Spanish", "es"),
    ("Italian", "it"),
    ("Dutch", "nl"),
    ("Swedish", "sv"),
    ("Danish", "da"),
    ("Norwegian", "no"),
    ("Finnish", "fi"),
    ("Portuguese", "pt"),
    ("Korean", "ko"),
    ("Russian", "ru"),
    ("Polish", "pl"),
    ("Turkish", "tr"),
    ("Greek", "el"),
    ("Hebrew", "he"),
    ("Arabic", "ar"),
];

type Plist = Retained<NSDictionary<AnyObject, AnyObject>>;

struct Bundle {
    path: PathBuf,
    app: NamedApp,
}

/// The folders installed applications live in, system first so
/// `/Applications` wins over `~/Applications`, then Finder: the rest of
/// `CoreServices` is agents and helpers no one launches by name.
pub fn app_roots() -> Vec<PathBuf> {
    let home = PathBuf::from(NSHomeDirectory().to_string());
    vec![
        "/Applications".into(),
        "/System/Applications".into(),
        "/System/Applications/Utilities".into(),
        "/Applications/Utilities".into(),
        home.join("Applications"),
        home.join("Applications/Chrome Apps.localized"),
        "/System/Library/CoreServices/Finder.app".into(),
    ]
}

/// The Mac's languages, first choice first (`fr-FR`, `en-US`).
pub fn preferred_languages() -> Vec<String> {
    NSLocale::preferredLanguages()
        .iter()
        .map(|language| language.to_string())
        .collect()
}

/// The bundle in `roots` that `query` names in any language it ships, by an
/// exact name or the start of one; several equally good bundles are refused.
pub fn find(query: &str, roots: &[PathBuf], languages: &[String]) -> Result<PathBuf, NameMiss> {
    let bundles = scan(roots);
    let apps: Vec<NamedApp> = bundles.iter().map(|bundle| bundle.app.clone()).collect();
    let query = query.trim().trim_end_matches(".app");
    app_names::lookup(query, &apps, languages).map(|index| bundles[index].path.clone())
}

/// The `.app` bundles inside each root and one folder down (Adobe's
/// `Adobe InDesign 2026/Adobe InDesign 2026.app`), in root order, each once
/// (`/Applications/Utilities` is a root and one folder down). A root that is
/// itself a bundle is that one bundle.
fn scan(roots: &[PathBuf]) -> Vec<Bundle> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for root in roots {
        if is_app(root) {
            paths.extend(root.is_dir().then(|| root.clone()));
            continue;
        }
        let (apps, folders): (Vec<PathBuf>, Vec<PathBuf>) =
            sorted_dirs(root).into_iter().partition(|path| is_app(path));
        let nested = folders
            .iter()
            .flat_map(|folder| sorted_dirs(folder).into_iter().filter(|path| is_app(path)));
        paths.extend(apps.into_iter().chain(nested));
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths.into_iter().map(load_bundle).collect()
}

fn is_app(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "app")
}

/// The sub-directories of `folder`, sorted; a folder that cannot be read has
/// none, as a root that is not installed.
fn sorted_dirs(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn load_bundle(path: PathBuf) -> Bundle {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let info = read_plist(&path.join("Contents/Info.plist"));
    let own = info.as_ref().map_or_else(Vec::new, |info| named(info, ""));
    let title = own
        .first()
        .map_or_else(|| stem.clone(), |name| name.text.clone());
    let mut names = vec![AppName::new(stem, "")];
    names.extend(own);
    names.extend(localized_names(&path.join("Contents/Resources")));
    Bundle {
        path,
        app: NamedApp {
            title,
            names,
            id: info
                .as_ref()
                .and_then(|info| plist_string(info, "CFBundleIdentifier")),
        },
    }
}

/// Every language's shown names, from the loctable and the `.lproj` folders.
/// `Base` holds names for no one language, like the unlocalized metadata.
fn localized_names(resources: &Path) -> Vec<AppName> {
    let mut names = Vec::new();
    if let Some(table) = read_plist(&resources.join("InfoPlist.loctable")) {
        for language in &table.allKeys() {
            let Some(language) = language.downcast_ref::<NSString>() else {
                continue;
            };
            let Some(strings) = table
                .objectForKey(language)
                .and_then(|strings| strings.downcast::<NSDictionary>().ok())
            else {
                continue;
            };
            names.extend(named(&strings, &language_code(&language.to_string())));
        }
    }
    for folder in sorted_dirs(resources) {
        if folder.extension().is_none_or(|ext| ext != "lproj") {
            continue;
        }
        let stem = folder.file_stem().unwrap_or_default().to_string_lossy();
        if let Some(strings) = read_plist(&folder.join("InfoPlist.strings")) {
            names.extend(named(&strings, &language_code(&stem)));
        }
    }
    names
}

/// The language a loctable key or `.lproj` folder stands for: legacy names
/// mapped to their code, `Base` to none.
fn language_code(folder: &str) -> String {
    if folder == "Base" {
        return String::new();
    }
    LEGACY_LANGUAGES
        .iter()
        .find(|(legacy, _)| *legacy == folder)
        .map_or(folder, |(_, code)| *code)
        .to_owned()
}

fn named(strings: &Plist, language: &str) -> Vec<AppName> {
    NAME_KEYS
        .iter()
        .filter_map(|key| plist_string(strings, key))
        .map(|text| AppName::new(text, language))
        .collect()
}

/// The dictionary a property list file holds, in any format (XML, binary,
/// or an old-style strings file in UTF-8 or UTF-16). A bundle without the
/// file, or with one that is no dictionary, ships no names from it.
fn read_plist(path: &Path) -> Option<Plist> {
    let bytes = std::fs::read(path).ok()?;
    let data = NSData::with_bytes(&bytes);
    // SAFETY: a null format pointer is allowed; the format is not wanted.
    let object = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            NSPropertyListMutabilityOptions::Immutable,
            core::ptr::null_mut(),
        )
    }
    .ok()?;
    object.downcast::<NSDictionary>().ok()
}

/// The non-empty, trimmed string under `key`.
fn plist_string(plist: &Plist, key: &str) -> Option<String> {
    let value = plist.objectForKey(&NSString::from_str(key))?;
    let value = value.downcast_ref::<NSString>()?.to_string();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::find;
    use crate::app_names::NameMiss;
    use std::path::{Path, PathBuf};

    /// Writes an `.app` bundle with `info` inside its Info.plist dict.
    /// Nothing runs: the lookup only reads files.
    fn bundle(at: &Path, bundle_id: &str, info: &str) -> PathBuf {
        let contents = at.join("Contents");
        std::fs::create_dir_all(contents.join("Resources")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{bundle_id}</string>
{info}
</dict></plist>"#
            ),
        )
        .unwrap();
        at.to_path_buf()
    }

    fn langs(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| (*tag).to_owned()).collect()
    }

    #[test]
    fn an_app_one_folder_down_is_found_by_its_name() {
        let root = tempfile::tempdir().unwrap();
        let indesign = bundle(
            &root
                .path()
                .join("Adobe InDesign 2026/Adobe InDesign 2026.app"),
            "com.adobe.InDesign",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(
            find("Adobe InDesign 2026", &roots, &[]),
            Ok(indesign.clone())
        );
        assert_eq!(
            find("indesign", &roots, &[]).unwrap_err().suggestions,
            ["Adobe InDesign 2026"]
        );
        assert_eq!(find("adobe indesign", &roots, &[]), Ok(indesign));
    }

    #[test]
    fn the_shown_name_finds_a_bundle_whose_file_name_differs() {
        let root = tempfile::tempdir().unwrap();
        let bambu = bundle(
            &root.path().join("BambuStudio.app"),
            "com.bambulab.bambu-studio",
            "<key>CFBundleName</key><string>BambuStudio</string>",
        );
        let viewer = bundle(
            &root.path().join("PV.app"),
            "com.example.viewer",
            "<key>CFBundleDisplayName</key><string>Photo Viewer</string>",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(find("bambustudio", &roots, &[]), Ok(bambu));
        assert_eq!(find("photo viewer", &roots, &[]), Ok(viewer));
    }

    #[test]
    fn a_localized_name_from_a_loctable_or_strings_file_finds_the_app() {
        let root = tempfile::tempdir().unwrap();
        let chess = bundle(
            &root.path().join("Chess.app"),
            "com.apple.Chess",
            "<key>CFBundleName</key><string>Chess</string>",
        );
        std::fs::write(
            chess.join("Contents/Resources/InfoPlist.loctable"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>fr</key><dict><key>CFBundleDisplayName</key><string>Échecs</string></dict>
<key>zh_CN</key><dict><key>CFBundleDisplayName</key><string>国际象棋</string></dict>
<key>ja</key><dict><key>CFBundleDisplayName</key><string>チェス</string></dict>
<key>ko</key><dict><key>CFBundleDisplayName</key><string>체스</string></dict>
<key>LocProvenance</key><dict><key>fr</key><integer>1</integer></dict>
</dict></plist>"#,
        )
        .unwrap();
        let mail = bundle(&root.path().join("Mail.app"), "com.apple.mail", "");
        // A legacy folder name and an old-style UTF-16 strings file.
        let german = mail.join("Contents/Resources/German.lproj");
        std::fs::create_dir_all(&german).unwrap();
        let strings: Vec<u8> = "\u{FEFF}/* names */\n\"CFBundleDisplayName\" = \"Post\";\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        std::fs::write(german.join("InfoPlist.strings"), strings).unwrap();
        let roots = [root.path().to_path_buf()];

        assert_eq!(
            find("échecs", &roots, &langs(&["en-US"])),
            Ok(chess.clone())
        );
        assert_eq!(
            find("Échecs", &roots, &langs(&["fr-FR"])),
            Ok(chess.clone())
        );
        for name in ["国际象棋", "チェス", "체스"] {
            assert_eq!(find(name, &roots, &[]), Ok(chess.clone()), "{name}");
        }
        assert_eq!(find("post", &roots, &langs(&["de-DE"])), Ok(mail));
    }

    #[test]
    fn two_apps_answering_to_a_name_are_refused_with_suggestions() {
        let root = tempfile::tempdir().unwrap();
        bundle(
            &root
                .path()
                .join("Adobe InDesign 2025/Adobe InDesign 2025.app"),
            "com.adobe.InDesign.2025",
            "",
        );
        bundle(
            &root
                .path()
                .join("Adobe InDesign 2026/Adobe InDesign 2026.app"),
            "com.adobe.InDesign",
            "",
        );
        let roots = [root.path().to_path_buf()];
        assert_eq!(
            find("Adobe InDesign", &roots, &[]),
            Err(NameMiss {
                ambiguous: true,
                suggestions: vec![
                    "Adobe InDesign 2025".to_owned(),
                    "Adobe InDesign 2026".to_owned()
                ],
            })
        );
    }

    #[test]
    fn copies_of_one_bundle_id_launch_the_copy_the_name_belongs_to() {
        let system = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let first = bundle(&system.path().join("Notes.app"), "com.apple.Notes", "");
        let copy = bundle(&user.path().join("Notes copy.app"), "com.apple.Notes", "");
        let roots = [system.path().to_path_buf(), user.path().to_path_buf()];
        assert_eq!(find("notes", &roots, &[]), Ok(first));
        assert_eq!(find("notes copy", &roots, &[]), Ok(copy));
    }

    #[test]
    fn a_bundle_under_two_overlapping_roots_is_one_app() {
        let root = tempfile::tempdir().unwrap();
        let tool = root.path().join("Utilities/Tool.app");
        std::fs::create_dir_all(&tool).unwrap();
        let roots = [root.path().to_path_buf(), root.path().join("Utilities")];
        assert_eq!(find("tool", &roots, &[]), Ok(tool));
    }

    #[test]
    fn a_base_localization_name_finds_the_app() {
        let root = tempfile::tempdir().unwrap();
        let app = bundle(&root.path().join("PV.app"), "com.example.viewer", "");
        std::fs::write(
            app.join("Contents/Resources/InfoPlist.loctable"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Base</key><dict><key>CFBundleDisplayName</key><string>Photo Viewer</string></dict>
</dict></plist>"#,
        )
        .unwrap();
        let other = bundle(&root.path().join("Other.app"), "com.example.other", "");
        let base = other.join("Contents/Resources/Base.lproj");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("InfoPlist.strings"),
            "\"CFBundleDisplayName\" = \"Other Viewer\";\n",
        )
        .unwrap();
        let roots = [root.path().to_path_buf()];
        assert_eq!(find("photo viewer", &roots, &langs(&["en-US"])), Ok(app));
        assert_eq!(find("other viewer", &roots, &langs(&["en-US"])), Ok(other));
    }

    #[test]
    fn a_root_that_is_a_bundle_is_scanned_as_that_bundle() {
        let root = tempfile::tempdir().unwrap();
        let finder = bundle(
            &root.path().join("CoreServices/Finder.app"),
            "com.apple.finder",
            "",
        );
        bundle(
            &root.path().join("CoreServices/Siri.app"),
            "com.apple.Siri",
            "",
        );
        let roots = [finder.clone()];
        assert_eq!(find("finder", &roots, &[]), Ok(finder));
        assert!(find("siri", &roots, &[]).is_err());
    }

    /// Reads the installed system applications; nothing launches and no
    /// window shows.
    #[test]
    fn installed_system_apps_are_found_by_name() {
        let roots = super::app_roots();
        let languages = langs(&["en-US"]);
        let finder = find("Finder", &roots, &languages).expect("Finder is installed");
        assert_eq!(
            finder,
            PathBuf::from("/System/Library/CoreServices/Finder.app")
        );
        let calculator = find("calculator", &roots, &languages).expect("Calculator is installed");
        assert_eq!(
            calculator.file_name().and_then(|name| name.to_str()),
            Some("Calculator.app")
        );
    }
}
