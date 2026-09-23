//! Search results as the search screen shows them: everything, only the library, or only what the
//! providers offer (octo-fiesta marks provider items `isExternal`; Navidrome's are the rest). The split
//! is made once per answer, so switching between the three costs nothing.

use std::collections::HashSet;

use crate::{Core, Result, SearchResult};

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchSplit {
    pub everything: SearchResult,
    /// Only the library's items; None when that is everything (there are no provider items).
    pub library: Option<SearchResult>,
    /// Only the providers' items; None when there are none.
    pub providers: Option<SearchResult>,
    pub has_providers: bool,
}

fn split(r: SearchResult) -> SearchSplit {
    let has_providers = r.songs.iter().any(|s| s.is_external) || r.albums.iter().any(|a| a.is_external) || r.artists.iter().any(|a| a.is_external);
    if !has_providers {
        return SearchSplit { everything: r, library: None, providers: None, has_providers };
    }
    let part = |external: bool| SearchResult {
        artists: r.artists.iter().filter(|a| a.is_external == external).cloned().collect(),
        albums: r.albums.iter().filter(|a| a.is_external == external).cloned().collect(),
        songs: r.songs.iter().filter(|s| s.is_external == external).cloned().collect(),
    };
    let (library, providers) = (part(false), part(true));
    SearchSplit { everything: r, library: Some(library), providers: Some(providers), has_providers }
}

fn distinct<T>(list: Vec<T>, id: impl Fn(&T) -> &str) -> Vec<T> {
    let mut seen = HashSet::new();
    list.into_iter().filter(|x| seen.insert(id(x).to_string())).collect()
}

/// The server's answer, ready to show. A merged provider result may repeat an id, and lists are keyed by
/// id, so only the first of each is kept.
#[uniffi::export]
pub fn search_split(result: SearchResult) -> SearchSplit {
    split(SearchResult {
        artists: distinct(result.artists, |a| &a.id),
        albums: distinct(result.albums, |a| &a.id),
        songs: distinct(result.songs, |s| &s.id),
    })
}

/// Shorter than this, a query is a keystroke on the way to one, not one worth remembering.
const REMEMBER_MIN_UTF16: usize = 2;

#[uniffi::export]
impl Core {
    /// The offline index's answer to every keystroke, split like the server's (it never holds provider items).
    pub fn local_search_split(&self, query: String, limit: u32) -> Result<SearchSplit> {
        Ok(split(self.local_search(query, limit)?))
    }

    /// Remembers a query the user acted on and returns the history as it now is; None, and nothing
    /// remembered, when the query is too short to be one.
    pub fn search_remember_recent(&self, query: String) -> Result<Option<Vec<String>>> {
        if query.encode_utf16().count() < REMEMBER_MIN_UTF16 {
            return Ok(None);
        }
        self.search_remember(query)?;
        Ok(Some(self.search_history()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Album, Artist, Song};

    fn result() -> SearchResult {
        let s = |id: &str, ext: bool| Song { id: id.into(), is_external: ext, ..Default::default() };
        let a = |id: &str, ext: bool| Album { id: id.into(), is_external: ext, ..Default::default() };
        SearchResult {
            artists: vec![Artist { id: "ar".into(), ..Default::default() }],
            albums: vec![a("al", false), a("ext-al", true), a("ext-al", true)],
            songs: vec![s("1", false), s("ext-2", true), s("1", false), s("3", false)],
        }
    }

    fn ids<T>(l: &[T], id: impl Fn(&T) -> &str) -> Vec<&str> {
        l.iter().map(id).collect()
    }

    #[test]
    fn server_answers_are_deduplicated_and_split() {
        let r = search_split(result());
        assert!(r.has_providers);
        assert_eq!(ids(&r.everything.songs, |s| &s.id), ["1", "ext-2", "3"]);
        assert_eq!(ids(&r.everything.albums, |a| &a.id), ["al", "ext-al"]);
        let (lib, prov) = (r.library.unwrap(), r.providers.unwrap());
        assert_eq!(ids(&lib.songs, |s| &s.id), ["1", "3"]);
        assert_eq!(lib.artists.len(), 1);
        assert_eq!(ids(&prov.songs, |s| &s.id), ["ext-2"]);
        assert_eq!(ids(&prov.albums, |a| &a.id), ["ext-al"]);
        assert!(prov.artists.is_empty());
    }

    #[test]
    fn a_library_only_answer_is_not_copied() {
        let mut r = result();
        r.songs.retain(|s| !s.is_external);
        r.albums.retain(|a| !a.is_external);
        let s = search_split(r);
        assert!(!s.has_providers && s.library.is_none() && s.providers.is_none());
    }

    #[test]
    fn only_real_queries_are_remembered() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        assert_eq!(core.search_remember_recent("a".into()).unwrap(), None);
        assert!(core.search_history().unwrap().is_empty());
        // One emoji is two UTF-16 units, as the app has always counted it.
        assert_eq!(core.search_remember_recent("🎵".into()).unwrap(), Some(vec!["🎵".to_string()]));
        let history = core.search_remember_recent("dogs".into()).unwrap().unwrap();
        assert_eq!(history.len(), 2);
        assert!(history.contains(&"dogs".to_string()));
    }
}
