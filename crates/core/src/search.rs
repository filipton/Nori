//! Search as the core answers it, from its index or from the server. How results are split and shown is
//! nori-library's.

use crate::{Core, Result, SearchResult};

pub use nori_library::search::*;

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

    /// The server could not answer `query`, for `reason` (the client's own words for the failure): the
    /// offline answer stays, and the view says it fell back.
    pub fn failed(&self, query: String, reason: Option<String>) -> Option<SearchView> {
        let mut s = self.0.lock();
        if s.query() != query {
            return None;
        }
        s.searching = false;
        s.error = Some(SearchFallback { reason });
        Some(s.view())
    }

    pub fn scope(&self, scope: SearchScope) -> SearchView {
        let mut s = self.0.lock();
        s.scope = scope;
        s.view()
    }
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
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

/// Asked only in Rust, so not exported to Kotlin.
impl Core {
    /// The offline index's answer to every keystroke, split like the server's (it never holds provider items).
    pub fn local_search_split(&self, query: String, limit: u32) -> Result<SearchSplit> {
        Ok(split(self.local_search(query, limit)?))
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(failed.error, Some(SearchFallback { reason: Some("timeout".into()) }));
        let v = s.typed("  ".into());
        assert!(v.shown.is_none() && !v.searching && v.error.is_none());
        assert!(s.scope(SearchScope::Everything).scopes_offered == false);
        let none = s.typed("x".into());
        assert!(!none.nothing_found);
        let v = s.server("x".into(), SearchResult::default()).unwrap();
        assert!(v.nothing_found);
        assert_eq!(s.scope(SearchScope::Library).shown.unwrap().songs.len(), 0, "no provider items: the library is everything");
        assert_eq!(search_scopes()[2], SearchScope::Providers);
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
