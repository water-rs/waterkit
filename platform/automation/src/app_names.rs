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
// libs/cua-driver/rust/crates/cua-driver-core/src/app_names.rs.

//! Matching an application name a caller gives against every name an
//! installed application ships, in any language, so "Échecs" finds Chess on
//! any Mac.
//!
//! [`bundle_names`](crate::bundle_names) reads the names; this module only
//! ranks them. Names compare without case but otherwise as shipped:
//! forgiving how a user spells a name (accents, spaces) is the client's job.
//! An exact name wins over a name the query starts, a name in one of the
//! user's languages wins over the same rank in another, and a tie at the top
//! is refused with suggestions instead of guessed.

use icu_normalizer::DecomposingNormalizerBorrowed;

/// How many names a refusal suggests.
pub const SUGGESTION_LIMIT: usize = 5;

/// One name an application ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppName {
    pub(crate) text: String,
    /// The localization the name comes from (`fr`, `pt_BR`); empty for the
    /// file name and the unlocalized metadata, which belong to no language.
    pub(crate) language: String,
}

impl AppName {
    pub(crate) fn new(text: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            language: language.into(),
        }
    }
}

/// An installed application as the matcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedApp {
    /// The name shown when no localized name fits the user's languages.
    pub(crate) title: String,
    pub(crate) names: Vec<AppName>,
    /// The bundle id; copies of one application tie to the first copy, not
    /// to a refusal.
    pub(crate) id: Option<String>,
}

/// Why a name did not resolve to exactly one application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameMiss {
    /// Several applications match the name equally well.
    pub(crate) ambiguous: bool,
    /// The closest application names, best first, at most
    /// [`SUGGESTION_LIMIT`].
    pub(crate) suggestions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    NoMatch,
    Contains,
    Prefix,
    PrefixPreferred,
    Exact,
    ExactPreferred,
}

/// The text as names are compared: trimmed, without case, and canonically
/// decomposed, so a name stored decomposed (as macOS file names are) meets
/// the same name typed composed. Accents and spaces stay: "Échecs" meets
/// "ÉCHECS", not "echecs".
///
/// Case in Turkish pairs `ı` with `I` and `i` with `İ`, which the default
/// mapping can't undo from upper case ("AYARLARI" is "Ayarları"), so the
/// four forms of i count as one letter.
pub fn fold(text: &str) -> String {
    DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&text.trim().to_lowercase())
        .replace("i\u{307}", "i")
        .replace('ı', "i")
}

/// Whether a localization (`fr`, `pt_BR`, `zh_CN`) is one of the user's
/// preferred languages (`fr-FR`, `pt-BR`, `zh-Hans-CN`). A region-tagged
/// localization also matches a tag that puts a script between language and
/// region.
pub fn preferred(language: &str, languages: &[String]) -> bool {
    let code = language.replace('_', "-").to_lowercase();
    if code.is_empty() {
        return false;
    }
    let (tongue, region) = code.split_once('-').unwrap_or((&code, ""));
    languages.iter().any(|wanted| {
        let wanted = wanted.replace('_', "-").to_lowercase();
        if wanted == code || wanted.starts_with(&format!("{code}-")) {
            return true;
        }
        let mut parts = wanted.split('-');
        parts.next() == Some(tongue) && !region.is_empty() && parts.any(|part| part == region)
    })
}

fn rank(query: &str, app: &NamedApp, languages: &[String]) -> Rank {
    app.names
        .iter()
        .map(|name| {
            let folded = fold(&name.text);
            let liked = preferred(&name.language, languages);
            if folded == query {
                if liked {
                    Rank::ExactPreferred
                } else {
                    Rank::Exact
                }
            } else if folded.starts_with(query) {
                if liked {
                    Rank::PrefixPreferred
                } else {
                    Rank::Prefix
                }
            } else if folded.contains(query) {
                Rank::Contains
            } else {
                Rank::NoMatch
            }
        })
        .max()
        .unwrap_or(Rank::NoMatch)
}

/// The application's name in the first of the user's languages it has one
/// for, else its title.
fn display_name<'a>(app: &'a NamedApp, languages: &[String]) -> &'a str {
    languages
        .iter()
        .find_map(|language| {
            app.names
                .iter()
                .find(|name| preferred(&name.language, std::slice::from_ref(language)))
        })
        .map_or(app.title.as_str(), |name| name.text.as_str())
}

/// The index of the one application `query` names, by an exact name or the
/// start of one, or why there is none.
pub fn lookup(query: &str, apps: &[NamedApp], languages: &[String]) -> Result<usize, NameMiss> {
    let query = fold(query);
    if query.is_empty() {
        return Err(NameMiss {
            ambiguous: false,
            suggestions: Vec::new(),
        });
    }
    let mut ranked: Vec<(Rank, usize)> = apps
        .iter()
        .enumerate()
        .map(|(index, app)| (rank(&query, app, languages), index))
        .filter(|(rank, _)| *rank != Rank::NoMatch)
        .collect();
    // Stable, so equal ranks keep the adapter's (root) order.
    ranked.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));

    let shown: Vec<&str> = ranked
        .iter()
        .map(|(_, index)| display_name(&apps[*index], languages))
        .collect();
    let mut suggestions: Vec<String> = Vec::new();
    for (name, (_, index)) in shown.iter().zip(&ranked) {
        // Two applications shown under one name are told apart by their
        // titles.
        let shared = shown.iter().filter(|other| *other == name).count() > 1;
        let name = if shared {
            apps[*index].title.as_str()
        } else {
            name
        };
        if !suggestions.iter().any(|known| known == name) {
            suggestions.push(name.to_owned());
        }
    }
    suggestions.truncate(SUGGESTION_LIMIT);

    let top = ranked.first().map_or(Rank::NoMatch, |(rank, _)| *rank);
    let mut best: Vec<usize> = Vec::new();
    for (_, index) in ranked.iter().take_while(|(rank, _)| *rank == top) {
        let id = &apps[*index].id;
        if id.is_none() || !best.iter().any(|known| apps[*known].id == *id) {
            best.push(*index);
        }
    }
    match best.as_slice() {
        [one] if top >= Rank::Prefix => Ok(*one),
        _ => Err(NameMiss {
            ambiguous: top >= Rank::Prefix,
            suggestions,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(title: &str, names: &[(&str, &str)]) -> NamedApp {
        NamedApp {
            title: title.to_owned(),
            names: names
                .iter()
                .map(|(text, language)| AppName::new(*text, *language))
                .collect(),
            id: None,
        }
    }

    fn langs(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| (*tag).to_owned()).collect()
    }

    #[test]
    fn fold_drops_case_and_encoding_but_keeps_accents_and_spaces() {
        assert_eq!(fold("  Échecs "), fold("échecs"));
        // The same name, stored decomposed as macOS file names are.
        assert_eq!(fold("E\u{301}checs"), fold("Échecs"));
        assert_ne!(fold("Échecs"), fold("echecs"));
        assert_ne!(fold("Bambu Studio"), fold("BambuStudio"));
    }

    /// Names macOS ships in Turkish, in upper and lower case.
    #[test]
    fn turkish_dotted_and_dotless_i_are_one_letter_without_case() {
        assert_eq!(fold("Sistem Ayarları"), fold("SISTEM AYARLARI"));
        assert_eq!(fold("İpuçları"), fold("ipuçları"));
        assert_eq!(fold("İpuçları"), fold("İPUÇLARI"));
        assert_ne!(fold("Ayarları"), fold("Ayarlar"));
    }

    #[test]
    fn a_localized_name_finds_the_app_in_any_case() {
        let apps = [
            app("Calculator", &[("Calculator", "")]),
            app(
                "Chess",
                &[("Chess", ""), ("Échecs", "fr"), ("Schach", "de")],
            ),
        ];
        assert_eq!(lookup("Échecs", &apps, &langs(&["en-US"])), Ok(1));
        assert_eq!(lookup("ÉCHECS", &apps, &langs(&["fr-FR"])), Ok(1));
        assert_eq!(lookup("schach", &apps, &[]), Ok(1));
        // A spelling the app does not ship is the client's to forgive.
        assert!(!lookup("echec", &apps, &[]).unwrap_err().ambiguous);
    }

    #[test]
    fn cjk_names_match_in_their_own_language() {
        let apps = [
            app(
                "Chess",
                &[("Chess", ""), ("国际象棋", "zh_CN"), ("チェス", "ja")],
            ),
            app("Calculator", &[("Calculator", ""), ("계산기", "ko")]),
        ];
        assert_eq!(lookup("国际象棋", &apps, &langs(&["zh-Hans-CN"])), Ok(0));
        assert_eq!(lookup("チェス", &apps, &[]), Ok(0));
        assert_eq!(lookup("계산기", &apps, &langs(&["ko-KR"])), Ok(1));
    }

    #[test]
    fn an_exact_name_wins_over_a_longer_one_it_starts() {
        let apps = [
            app("Notes Plus", &[("Notes Plus", "")]),
            app("Notes", &[("Notes", "")]),
        ];
        assert_eq!(lookup("notes", &apps, &[]), Ok(1));
    }

    #[test]
    fn a_prefix_match_resolves_when_it_is_the_only_one() {
        let apps = [
            app("Google Chrome", &[("Google Chrome", "")]),
            app("Calculator", &[("Calculator", "")]),
        ];
        assert_eq!(lookup("google", &apps, &[]), Ok(0));
    }

    #[test]
    fn the_users_language_breaks_an_exact_tie() {
        let apps = [
            app("Mail", &[("Mail", ""), ("Post", "de")]),
            app("Post", &[("Post", "")]),
        ];
        assert_eq!(lookup("post", &apps, &langs(&["de-DE"])), Ok(0));
        let refused = lookup("post", &apps, &langs(&["en-US"])).unwrap_err();
        assert!(refused.ambiguous);
    }

    #[test]
    fn the_users_language_breaks_a_prefix_tie() {
        let apps = [
            app("Postfach", &[("Postfach", "")]),
            app("Mail", &[("Mail", ""), ("Postamt", "de")]),
        ];
        assert_eq!(lookup("post", &apps, &langs(&["de"])), Ok(1));
        assert!(lookup("post", &apps, &[]).unwrap_err().ambiguous);
    }

    #[test]
    fn a_tie_is_refused_with_both_names_never_guessed() {
        let apps = [
            app("Adobe InDesign 2025", &[("Adobe InDesign 2025", "")]),
            app("Adobe InDesign 2026", &[("Adobe InDesign 2026", "")]),
        ];
        assert_eq!(
            lookup("adobe indesign", &apps, &[]),
            Err(NameMiss {
                ambiguous: true,
                suggestions: vec![
                    "Adobe InDesign 2025".to_owned(),
                    "Adobe InDesign 2026".to_owned()
                ],
            })
        );
        assert_eq!(lookup("Adobe InDesign 2026", &apps, &[]), Ok(1));
    }

    #[test]
    fn copies_of_one_app_tie_to_the_first_copy() {
        let copy = |names: &[(&str, &str)]| NamedApp {
            id: Some("com.example.notes".to_owned()),
            ..app("Notes", names)
        };
        let apps = [
            copy(&[("Notes", "")]),
            copy(&[("Notes", ""), ("Notes Beta", "")]),
        ];
        assert_eq!(lookup("notes", &apps, &[]), Ok(0));
        // A name only the second copy ships launches that copy.
        assert_eq!(lookup("notes beta", &apps, &[]), Ok(1));
    }

    #[test]
    fn a_name_found_only_inside_another_is_a_suggestion_not_a_match() {
        let apps = [app("Keynote", &[("Keynote", "")])];
        assert_eq!(
            lookup("note", &apps, &[]),
            Err(NameMiss {
                ambiguous: false,
                suggestions: vec!["Keynote".to_owned()],
            })
        );
    }

    #[test]
    fn an_unknown_or_empty_name_has_no_suggestions() {
        let apps = [app("Keynote", &[("Keynote", "")])];
        let none = NameMiss {
            ambiguous: false,
            suggestions: Vec::new(),
        };
        assert_eq!(lookup("zzz", &apps, &[]), Err(none.clone()));
        assert_eq!(lookup("  ", &apps, &[]), Err(none));
    }

    #[test]
    fn suggestions_stop_at_the_limit() {
        let apps: Vec<NamedApp> = (0..SUGGESTION_LIMIT + 3)
            .map(|index| app(&format!("Tool {index}"), &[(&format!("Tool {index}"), "")]))
            .collect();
        let refused = lookup("tool", &apps, &[]).unwrap_err();
        assert!(refused.ambiguous);
        assert_eq!(refused.suggestions.len(), SUGGESTION_LIMIT);
    }

    #[test]
    fn suggestions_use_the_users_language_and_titles_for_shared_names() {
        let apps = [
            app("Chess", &[("Chess", ""), ("Échecs", "fr")]),
            app("Chess Pro", &[("Chess Pro", ""), ("Échecs", "fr")]),
        ];
        let refused = lookup("ess", &apps, &langs(&["fr-FR"])).unwrap_err();
        assert_eq!(refused.suggestions, ["Chess", "Chess Pro"]);
        let apps = [app("Chess", &[("Chess", ""), ("Échecs", "fr")])];
        let refused = lookup("ess", &apps, &langs(&["fr-FR"])).unwrap_err();
        assert_eq!(refused.suggestions, ["Échecs"]);
    }

    #[test]
    fn preferred_matches_folders_against_language_tags() {
        assert!(preferred("fr", &langs(&["fr-FR"])));
        assert!(preferred("pt_BR", &langs(&["pt-BR"])));
        assert!(preferred("zh_CN", &langs(&["zh-Hans-CN"])));
        assert!(!preferred("pt_PT", &langs(&["pt-BR"])));
        assert!(!preferred("", &langs(&["en"])));
    }
}
