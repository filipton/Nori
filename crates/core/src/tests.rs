use super::*;

const SEARCH: &str = r#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"navidrome","openSubsonic":true,"searchResult3":{
 "artist":[{"id":"ar1","name":"Pink Floyd","albumCount":2,"starred":"2024-01-01T00:00:00Z"}],
 "album":[{"id":"al1","name":"Animals","artist":"Pink Floyd","songCount":5},{"id":"pl-deezer-9","name":"Mix","isExternal":true}],
 "song":[{"id":"s1","title":"Dogs","artist":"Pink Floyd","album":"Animals","duration":1024,"replayGain":{"trackGain":-6.5},"discNumber":1},
         {"id":"ext-deezer-song-7","title":"Pigs","artist":"Pink Floyd","isExternal":true},
         {"id":42,"title":"Numeric"}]}}}"#;

#[test]
fn search_parses_indexes_and_skips_external() {
    let core = Core::new(String::new()).unwrap();
    let r = core.parse_search(SEARCH.into()).unwrap();
    assert_eq!(r.songs.len(), 3);
    assert!(r.artists[0].starred);
    assert!(r.songs[1].is_external);
    assert_eq!(r.songs[2].id, "42");
    assert_eq!(r.songs[0].replay_gain.as_ref().unwrap().track_gain, Some(-6.5));

    let size = core.index_size().unwrap();
    assert_eq!((size.artists, size.albums, size.songs), (1, 1, 2));

    let l = core.local_search("pin flo do".into(), 10).unwrap();
    assert_eq!(l.songs.len(), 1);
    assert_eq!(l.songs[0], r.songs[0]);
    assert_eq!(core.local_search("pigs".into(), 10).unwrap().songs.len(), 0);
    assert_eq!(core.local_search("\"' OR *".into(), 10).unwrap().songs.len(), 0);

    // same page again changes nothing but still reports what it saw
    assert_eq!(core.ingest_search(SEARCH.into()).unwrap().songs, 3);
    assert_eq!(core.index_size().unwrap().songs, 2);
}

#[test]
fn api_error_is_surfaced() {
    let core = Core::new(String::new()).unwrap();
    let e = core.parse_status(r#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"Wrong username or password"}}}"#.into());
    assert!(matches!(e, Err(CoreError::Api { code: 40, .. })));
}

#[test]
fn queue_and_cache_round_trip() {
    let core = Core::new(String::new()).unwrap();
    let songs = core.parse_search(SEARCH.into()).unwrap().songs;
    core.save_queue(PlayQueue { songs: songs.clone(), index: 1, position_ms: 5000 }).unwrap();
    let q = core.load_queue().unwrap();
    assert_eq!((q.songs, q.index, q.position_ms), (songs, 1, 5000));

    core.cache_put("getAlbum?id=1".into(), vec![1, 2]).unwrap();
    core.cache_put("getArtist?id=1".into(), vec![3]).unwrap();
    assert!(core.cache_fresh("getAlbum?id=1".into(), 60_000).unwrap());
    assert!(!core.cache_fresh("getAlbum?id=1".into(), 0).unwrap());
    assert!(!core.cache_fresh("missing".into(), 60_000).unwrap());
    core.cache_evict("getAlbum".into()).unwrap();
    assert_eq!(core.cache_get("getAlbum?id=1".into()).unwrap(), None);
    assert_eq!(core.cache_get("getArtist?id=1".into()).unwrap(), Some(vec![3]));
}

#[test]
fn synced_lyrics_preferred() {
    let core = Core::new(String::new()).unwrap();
    let l = core
        .parse_lyrics(r#"{"subsonic-response":{"status":"ok","lyricsList":{"structuredLyrics":[
          {"synced":false,"line":[{"value":"plain"}]},{"synced":true,"line":[{"start":1500,"value":"timed"}]}]}}}"#.into())
        .unwrap();
    assert!(l.synced);
    assert_eq!((l.lines[0].start_ms, l.lines[0].text.as_str()), (1500, "timed"));
}
