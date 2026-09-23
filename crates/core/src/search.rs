//! Search results as the search screen shows them: everything, only the library, or only what the
//! providers offer (octo-fiesta marks provider items `isExternal`; Navidrome's are the rest). The split
//! is made once per answer, so switching between the three costs nothing.

use std::collections::HashSet;

use crate::{Core, Result, SearchResult};

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
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
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn search_split(result: SearchResult) -> SearchSplit {
    split(SearchResult {
        artists: distinct(result.artists, |a| &a.id),
        albums: distinct(result.albums, |a| &a.id),
        songs: distinct(result.songs, |s| &s.id),
    })
}

/// Which of the answer the search screen shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SearchScope {
    #[default]
    Everything,
    /// Only what is in the library already; everything when there are no provider items.
    Library,
    /// Only what the providers offer; nothing when there is none.
    Providers,
}

/// The scope chips and their words, in their order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SearchScopeChip {
    pub scope: SearchScope,
    pub label: String,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn search_scopes() -> Vec<SearchScopeChip> {
    [(SearchScope::Everything, "Everything"), (SearchScope::Library, "In library"), (SearchScope::Providers, "Not in library yet")]
        .map(|(scope, label)| SearchScopeChip { scope, label: label.into() })
        .to_vec()
}

impl SearchSplit {
    /// The answer narrowed to `scope`.
    fn shown(&self, scope: SearchScope) -> SearchResult {
        match scope {
            SearchScope::Everything => self.everything.clone(),
            SearchScope::Library => self.library.clone().unwrap_or_else(|| self.everything.clone()),
            SearchScope::Providers => self.providers.clone().unwrap_or_default(),
        }
    }
}

/// What the search screen shows now.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SearchView {
    /// What is in the field, as typed.
    pub text: String,
    /// The query that is asked: the text trimmed. Empty for none.
    pub query: String,
    /// The answer narrowed to the scope; None with no query (the recent searches show instead).
    pub shown: Option<SearchResult>,
    /// The answer is the server's rather than the offline index's.
    pub from_server: bool,
    /// The server is being asked.
    pub searching: bool,
    /// The server could not be asked, in words; the offline answer stays on screen.
    pub error: Option<String>,
    pub scope: SearchScope,
    /// The scope chips are offered: there are provider items, or a scope other than everything is on.
    pub scopes_offered: bool,
    /// The server answered with nothing at all.
    pub nothing_found: bool,
}

#[derive(Default)]
struct Session {
    text: String,
    split: Option<SearchSplit>,
    from_server: bool,
    searching: bool,
    error: Option<String>,
    scope: SearchScope,
}

impl Session {
    fn query(&self) -> &str {
        self.text.trim()
    }

    fn view(&self) -> SearchView {
        let shown = self.split.as_ref().map(|s| s.shown(self.scope));
        let has_providers = self.split.as_ref().is_some_and(|s| s.has_providers);
        let empty = shown.as_ref().is_some_and(|r| r.songs.is_empty() && r.albums.is_empty() && r.artists.is_empty());
        SearchView {
            text: self.text.clone(),
            query: self.query().to_string(),
            nothing_found: self.from_server && empty,
            shown,
            from_server: self.from_server,
            searching: self.searching,
            error: self.error.clone(),
            scope: self.scope,
            scopes_offered: has_providers || self.scope != SearchScope::Everything,
        }
    }
}

/// Live search in two layers, as one state: every keystroke is answered at once from the offline index,
/// and once typing pauses the server is asked too, because only the server (octo-fiesta) knows what is
/// not in the library yet. An answer is taken only while it is still the answer to what is in the field,
/// and the index's never over the server's for the same query - both come back out of order.
#[derive(Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Object))]
pub struct SearchSession(parking_lot::Mutex<Session>);

#[cfg_attr(feature = "ffi", uniffi::export)]
impl SearchSession {
    #[cfg_attr(feature = "ffi", uniffi::constructor)]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    /// The field now says `text`. Emptied, it goes back to the recent searches.
    pub fn typed(&self, text: String) -> SearchView {
        let mut s = self.0.lock();
        let blank = text.trim().is_empty();
        s.text = text;
        s.from_server = false;
        s.error = None;
        s.searching = !blank;
        if blank {
            s.split = None;
        }
        s.view()
    }

    /// The offline index's answer to `query`, when it is still the one in the field and the server has
    /// not answered it already; None otherwise.
    pub fn local(&self, core: std::sync::Arc<Core>, query: String, limit: u32) -> Result<Option<SearchView>> {
        let split = core.local_search_split(query.clone(), limit)?;
        let mut s = self.0.lock();
        if s.query() != query || s.from_server {
            return Ok(None);
        }
        s.split = Some(split);
        Ok(Some(s.view()))
    }

    /// The server's answer to `query`; None when the field has moved on.
    pub fn server(&self, query: String, result: SearchResult) -> Option<SearchView> {
        let split = search_split(result);
        let mut s = self.0.lock();
        if s.query() != query {
            return None;
        }
        s.split = Some(split);
        s.from_server = true;
        s.searching = false;
        s.error = None;
        Some(s.view())
    }

    /// Asks the server for `query` and takes its answer as [`SearchSession::server`] does, so the answer
    /// goes from the socket into the session without crossing to the platform and back. None when the
    /// field has moved on; an error when the server could not be asked ([`SearchSession::failed`]).
    pub async fn ask(&self, client: std::sync::Arc<crate::client::Client>, query: String) -> crate::client::NetResult<Option<SearchView>> {
        let sizes = crate::browse::library_sizes();
        let read = crate::cache_policy::Read::Search { query: query.clone(), songs: sizes.search_songs, albums: sizes.search_albums, artists: sizes.search_artists };
        let crate::cache_policy::Page::Found { v } = client.read_now(read).await? else { return Ok(None) };
        Ok(self.server(query, v))
    }

    /// The server could not answer `query`: said, and the offline answer stays.
    pub fn failed(&self, query: String, reason: Option<String>) -> Option<SearchView> {
        let mut s = self.0.lock();
        if s.query() != query {
            return None;
        }
        s.searching = false;
        s.error = Some(format!("Server search failed: {} — showing offline results", reason.as_deref().unwrap_or("null")));
        Some(s.view())
    }

    pub fn scope(&self, scope: SearchScope) -> SearchView {
        let mut s = self.0.lock();
        s.scope = scope;
        s.view()
    }
}

/// Shorter than this, a query is a keystroke on the way to one, not one worth remembering.
const REMEMBER_MIN_UTF16: usize = 2;

#[cfg_attr(feature = "ffi", uniffi::export)]
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
    fn a_session_takes_only_answers_to_what_is_in_the_field() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let s = SearchSession::new();
        let v = s.typed(" dogs ".into());
        assert_eq!((v.query.as_str(), v.searching, v.shown.is_none()), ("dogs", true, true));
        assert!(s.local(core.clone(), "dog".into(), 30).unwrap().is_none(), "an older keystroke");
        assert!(s.local(core.clone(), "dogs".into(), 30).unwrap().is_some());
        let v = s.server("dogs".into(), result()).unwrap();
        assert!(v.from_server && !v.searching && v.scopes_offered && !v.nothing_found);
        assert_eq!(v.shown.as_ref().unwrap().songs.len(), 3);
        assert!(s.local(core, "dogs".into(), 30).unwrap().is_none(), "the index never over the server");
        assert_eq!(s.scope(SearchScope::Providers).shown.unwrap().songs.len(), 1);
        assert_eq!(s.scope(SearchScope::Library).shown.unwrap().songs.len(), 2);
        assert!(s.server("cats".into(), result()).is_none());
        let failed = s.failed("dogs".into(), Some("timeout".into())).unwrap();
        assert_eq!(failed.error.as_deref(), Some("Server search failed: timeout — showing offline results"));
        let v = s.typed("  ".into());
        assert!(v.shown.is_none() && !v.searching && v.error.is_none());
        assert!(s.scope(SearchScope::Everything).scopes_offered == false);
        let none = s.typed("x".into());
        assert!(!none.nothing_found);
        let v = s.server("x".into(), SearchResult::default()).unwrap();
        assert!(v.nothing_found);
        assert_eq!(s.scope(SearchScope::Library).shown.unwrap().songs.len(), 0, "no provider items: the library is everything");
        assert_eq!(search_scopes()[2].label, "Not in library yet");
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
