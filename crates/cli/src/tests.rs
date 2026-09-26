//! The screen drawn into ratatui's TestBackend, and keys and clicks dispatched, with no server, no sound
//! card and no terminal. `NORI_TUI_DUMP=<dir>` also writes each screen drawn here as text.

use std::time::{Duration, Instant};

use nori_core::settings::StoredPrefs;
use nori_core::{Album, Song};
use nori_engine::State;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::app::{App, Cmd, Hit, ListRef, Overlay, Screen};
use crate::backend::{Data, Msg, Req};
use crate::settings_view::{Facts, Line, Row, SettingsView, GROUPS};

fn app() -> App {
    let mut a = App::new(StoredPrefs::default());
    a.server = "music.example".into();
    a
}

fn draw(a: &mut App, w: u16, h: u16) -> String {
    draw_with(a, w, h, None)
}

fn draw_with(a: &mut App, w: u16, h: u16, art: Option<&mut crate::art::Art>) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| crate::ui::draw(f, a, art)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// The screen as text, kept where NORI_TUI_DUMP says (for showing people what it looks like).
fn dump(name: &str, text: &str) {
    if let Some(dir) = std::env::var_os("NORI_TUI_DUMP") {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{name}.txt")), text);
    }
}

fn key(a: &mut App, code: KeyCode) {
    a.handle(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn chars(a: &mut App, s: &str) {
    for c in s.chars() {
        key(a, KeyCode::Char(c));
    }
}

fn click(a: &mut App, x: u16, y: u16) {
    let m = |kind| MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
    a.handle(Msg::Mouse(m(MouseEventKind::Down(MouseButton::Left))));
    a.handle(Msg::Mouse(m(MouseEventKind::Up(MouseButton::Left))));
}

fn hit_rect(a: &App, h: Hit) -> Rect {
    a.hits.iter().find(|(_, x)| *x == h).map(|(r, _)| *r).unwrap_or_else(|| panic!("{h:?} is not on screen"))
}

fn song(id: &str, title: &str, secs: u32) -> Song {
    Song { id: id.into(), title: title.into(), artist: "Artist".into(), album: "Album".into(), duration: secs, ..Default::default() }
}

fn album(id: &str, name: &str) -> Album {
    Album { id: id.into(), name: name.into(), artist: "Someone".into(), year: 2001, ..Default::default() }
}

#[test]
fn the_frame_has_the_tabs_the_player_bar_and_the_status_line() {
    let mut a = app();
    let s = draw(&mut a, 120, 30);
    for word in ["nori", "1 Home", "2 Library", "9 Settings", "Nothing playing", "q quit", "music.example"] {
        assert!(s.contains(word), "{word} missing:\n{s}");
    }
    dump("empty", &s);
}

#[test]
fn a_tiny_terminal_draws_without_panicking() {
    let mut a = app();
    for (w, h) in [(20, 6), (40, 10), (1, 1), (80, 4)] {
        for n in 0..9 {
            a.do_action(crate::keys::Action::Screen(n));
            draw(&mut a, w, h);
        }
    }
}

#[test]
fn keys_move_between_screens_and_ask_for_what_they_show() {
    let mut a = app();
    key(&mut a, KeyCode::Char('2'));
    assert_eq!(a.screen, Screen::Library);
    assert!(a.cmds.contains(&Cmd::Load(Req::Albums { offset: 0 })));
    key(&mut a, KeyCode::Char(']'));
    assert!(a.cmds.contains(&Cmd::Load(Req::Artists)));
    a.cmds.clear();
    key(&mut a, KeyCode::Char(' '));
    key(&mut a, KeyCode::Char('n'));
    key(&mut a, KeyCode::Char('p'));
    key(&mut a, KeyCode::Char('+'));
    assert_eq!(a.cmds, [Cmd::Toggle, Cmd::Next, Cmd::Previous, Cmd::Volume(1.0)]);
    key(&mut a, KeyCode::Char('-'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Volume(0.95)));
    // The search field takes every key while it is typed in, and the server is asked once typing pauses.
    key(&mut a, KeyCode::Char('/'));
    assert_eq!(a.screen, Screen::Search);
    chars(&mut a, "n q");
    assert_eq!(a.search.text, "n q", "n and q are letters in the field");
    assert!(!a.quit);
    assert_eq!(a.cmds.last(), Some(&Cmd::SearchTyped("n q".into())));
    let due = a.search.ask_at.expect("the server is asked later");
    a.tick(due + Duration::from_millis(1));
    assert_eq!(a.cmds.last(), Some(&Cmd::SearchServer("n q".into())));
    key(&mut a, KeyCode::Esc);
    key(&mut a, KeyCode::Char('?'));
    assert!(matches!(a.overlay, Some(Overlay::Help { .. })));
    let s = draw(&mut a, 100, 60);
    assert!(s.contains("Play or pause") && s.contains("Take it out of the queue"), "{s}");
    dump("help", &s);
    key(&mut a, KeyCode::Char('x'));
    assert!(a.overlay.is_none());
    key(&mut a, KeyCode::Char('q'));
    assert!(a.quit);
}

#[test]
fn seeking_needs_a_song_and_stays_inside_it() {
    let mut a = app();
    key(&mut a, KeyCode::Right);
    assert!(a.cmds.is_empty(), "nothing playing, nothing to seek");
    a.song = Some(song("1", "One", 200));
    a.now.state = State::Paused;
    a.now.position_ms = 3_000;
    key(&mut a, KeyCode::Left);
    assert_eq!(a.cmds.last(), Some(&Cmd::Seek(0)));
    a.handle(Msg::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)));
    assert_eq!(a.cmds.last(), Some(&Cmd::Seek(30_000)));
}

#[test]
fn a_list_is_clicked_scrolled_and_opened_with_the_mouse() {
    let mut a = app();
    a.go(Screen::Library);
    let albums: Vec<Album> = (0..100).map(|i| album(&format!("al-{i}"), &format!("Album {i}"))).collect();
    a.handle(Msg::Data(Req::Albums { offset: 0 }, Ok(Data::Albums(albums))));
    let s = draw(&mut a, 100, 30);
    assert!(s.contains("Album 0") && s.contains("Someone") && s.contains("2001"), "{s}");
    dump("library", &s);
    // A click selects the row, a second click on it opens the album.
    let row = hit_rect(&a, Hit::Row(ListRef::Library, 3));
    a.cmds.clear();
    click(&mut a, row.x + 2, row.y);
    assert_eq!(a.library.sels[0].at, 3);
    assert!(a.page().is_none());
    click(&mut a, row.x + 2, row.y);
    assert!(a.page().is_some(), "opened");
    assert!(a.cmds.contains(&Cmd::Load(Req::Album("al-3".into()))));
    key(&mut a, KeyCode::Esc);
    assert!(a.page().is_none());
    // The wheel moves the list under the pointer.
    draw(&mut a, 100, 30);
    let list = hit_rect(&a, Hit::List(ListRef::Library));
    let wheel = MouseEvent { kind: MouseEventKind::ScrollDown, column: list.x + 1, row: list.y + 1, modifiers: KeyModifiers::NONE };
    a.handle(Msg::Mouse(wheel));
    assert_eq!(a.library.sels[0].at, 6);
    // Tabs are clicked too.
    let tab = hit_rect(&a, Hit::Tab(3));
    click(&mut a, tab.x + 1, tab.y);
    assert_eq!(a.screen, Screen::Queue);
}

#[test]
fn the_seek_bar_is_clicked_and_dragged() {
    let mut a = app();
    a.song = Some(song("1", "One", 200));
    a.now.state = State::Playing;
    a.now.at = Instant::now();
    let s = draw(&mut a, 120, 30);
    assert!(s.contains("One") && s.contains("3:20"), "{s}");
    let bar = hit_rect(&a, Hit::Seek);
    let at = |share: f32| bar.x + (bar.width as f32 * share) as u16;
    let m = |kind, x| Msg::Mouse(MouseEvent { kind, column: x, row: bar.y, modifiers: KeyModifiers::NONE });
    a.handle(m(MouseEventKind::Down(MouseButton::Left), at(0.25)));
    a.handle(m(MouseEventKind::Drag(MouseButton::Left), at(0.5)));
    assert!(a.scrub.is_some_and(|s| (s - 0.5).abs() < 0.02), "the bar follows the drag");
    assert!(!a.cmds.iter().any(|c| matches!(c, Cmd::Seek(_))), "nothing is sought while dragging");
    a.handle(m(MouseEventKind::Up(MouseButton::Left), at(0.5)));
    let Some(Cmd::Seek(ms)) = a.cmds.last() else { panic!("{:?}", a.cmds) };
    assert!((ms - 100_000).abs() < 3_000, "{ms}");
    // Buttons in the bar.
    let next = hit_rect(&a, Hit::Button(crate::app::Button::Next));
    click(&mut a, next.x + 1, next.y);
    assert_eq!(a.cmds.last(), Some(&Cmd::Next));
}

#[test]
fn with_the_mouse_off_clicks_do_nothing() {
    let mut a = app();
    draw(&mut a, 100, 30);
    key(&mut a, KeyCode::Char('m'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Mouse(false)));
    let tab = hit_rect(&a, Hit::Tab(8));
    click(&mut a, tab.x + 1, tab.y);
    assert_eq!(a.screen, Screen::Home);
}

#[test]
fn every_settings_page_is_drawn_with_its_rows() {
    let mut a = app();
    a.go(Screen::Settings);
    a.settings.set_facts(Facts { devices: vec!["hw:0".into()], ..Facts::default() });
    for (i, g) in GROUPS.iter().enumerate() {
        a.settings.pane = 0;
        a.settings.group.at = i;
        a.settings.invalidate();
        let s = draw(&mut a, 160, 70);
        dump(&format!("settings-{}", g.id), &s);
        assert!(s.contains(g.title), "{}: group missing", g.id);
        let page = a.settings.page(&a.prefs.clone()).clone();
        for section in &page.sections {
            if !section.title.is_empty() {
                assert!(s.contains(&section.title), "{}: section {} missing", g.id, section.title);
            }
            for row in &section.rows {
                let title = match row {
                    Row::Toggle { title, .. } | Row::Choice { title, .. } | Row::Link { title, .. } | Row::Action { title, .. } => title.trim(),
                    _ => continue,
                };
                assert!(s.contains(title), "{}: {title} missing:\n{s}", g.id);
            }
        }
    }
    // What only a phone can do is not listed here.
    for i in 0..GROUPS.len() {
        a.settings.group.at = i;
        a.settings.invalidate();
        let s = draw(&mut a, 160, 70);
        for phone in ["System audio effects", "Save battery", "Swipe", "Moving covers", "Black background"] {
            assert!(!s.contains(phone), "{phone} listed in a terminal:\n{s}");
        }
    }
    // A switch shows its state, and enter on it asks the core to change it by its own name.
    a.settings.group.at = GROUPS.iter().position(|g| g.id == "sound").unwrap();
    a.settings.pane = 0;
    a.settings.invalidate();
    draw(&mut a, 160, 70);
    key(&mut a, KeyCode::Enter);
    assert_eq!(a.settings.pane, 1);
    a.cmds.clear();
    // Down to the AutoMix switch, counting the lines above it (titles are skipped over).
    let page = a.settings.page(&a.prefs.clone()).clone();
    let lines = SettingsView::lines(&page);
    let at = lines.iter().position(|l| matches!(l, Line::Row(Row::Toggle { name, .. }) if name == "autoMix")).unwrap();
    while a.settings.row.at < at {
        key(&mut a, KeyCode::Down);
    }
    key(&mut a, KeyCode::Enter);
    assert_eq!(a.cmds.last(), Some(&Cmd::Setting("autoMix".into(), "true".into())));
    // ← and → change a choice (the crossfade, just above); here they do not seek.
    key(&mut a, KeyCode::Up);
    a.cmds.clear();
    key(&mut a, KeyCode::Right);
    assert!(matches!(a.cmds.last(), Some(Cmd::Setting(n, _)) if n == "crossfadeSec"), "{:?}", a.cmds);
    // The output device is the terminal's own: kept for the next start.
    a.settings.group.at = 0;
    a.settings.pane = 0;
    a.settings.invalidate();
    draw(&mut a, 160, 70);
    key(&mut a, KeyCode::Enter);
    let page = a.settings.page(&a.prefs.clone()).clone();
    let lines = SettingsView::lines(&page);
    let at = lines.iter().position(|l| matches!(l, Line::Row(Row::Choice { name, .. }) if name == "!device")).unwrap();
    while a.settings.row.at < at {
        key(&mut a, KeyCode::Down);
    }
    a.cmds.clear();
    key(&mut a, KeyCode::Right);
    assert_eq!(a.cmds.last(), Some(&Cmd::Device("hw:0".into())));
}

#[test]
fn a_page_that_would_not_load_says_why() {
    let mut a = app();
    a.go(Screen::Library);
    a.handle(Msg::Data(Req::Albums { offset: 0 }, Err("The server did not answer: HTTP 522".into())));
    let s = draw(&mut a, 100, 24);
    assert!(s.contains("Could not load this page") && s.contains("HTTP 522") && s.contains("R tries again"), "{s}");
    dump("error", &s);
    key(&mut a, KeyCode::Char('R'));
    assert!(a.cmds.contains(&Cmd::Load(Req::Albums { offset: 0 })));
    a.unreachable = Some("The server did not answer: HTTP 522".into());
    let s = draw(&mut a, 100, 24);
    assert!(s.contains("unreachable"), "{s}");
}

#[test]
fn nothing_wakes_the_screen_while_nothing_plays() {
    let mut a = app();
    let now = Instant::now();
    assert_eq!(a.next_wake(now), None, "idle: no timer at all");
    a.now.state = State::Paused;
    assert_eq!(a.next_wake(now), None, "paused: no timer at all");
    a.now = crate::app::Now { state: State::Playing, position_ms: 12_300, at: now, speed: 1.0, mixing: false, buffering: false };
    let wake = a.next_wake(now).unwrap();
    let ms = wake.duration_since(now).as_millis();
    assert!((690..=710).contains(&ms), "the next whole second of the song: {ms}");
    a.now.buffering = true;
    assert_eq!(a.next_wake(now), None, "waiting for the network: the clock stands still");
}

#[test]
fn the_queue_is_listed_in_play_order_and_edited_with_keys() {
    let mut a = app();
    let songs: Vec<Song> = (0..5).map(|i| song(&format!("s{i}"), &format!("Song {i}"), 100)).collect();
    a.queue = Some(nori_core::playlist::PlaylistView { songs, len: 5, list_rev: 1, order: vec![0, 1, 2, 3, 4], queued: vec![3], index: 1, shuffle: false, repeat: 0, bridging: false, rev: 1 });
    a.go(Screen::Queue);
    let s = draw(&mut a, 100, 20);
    assert!(s.contains("▶ Song 1") && s.contains("+ Song 3"), "{s}");
    dump("queue", &s);
    assert_eq!(a.queue_sel.at, 1, "the queue opens on the song playing");
    key(&mut a, KeyCode::Char('j'));
    key(&mut a, KeyCode::Char('d'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Remove(2)));
    key(&mut a, KeyCode::Char('K'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Move(2, 1)));
    key(&mut a, KeyCode::Enter);
    assert_eq!(a.cmds.last(), Some(&Cmd::Jump(1)));
    key(&mut a, KeyCode::Char('s'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Shuffle(true)));
    key(&mut a, KeyCode::Char('r'));
    assert_eq!(a.cmds.last(), Some(&Cmd::Repeat(2)), "off, then all");
}

#[test]
fn the_player_shows_the_song_heard_and_how_the_next_comes_in() {
    let mut a = app();
    let next = song("s2", "Second", 180);
    nori_core::queue::queue_register(vec![next.clone()]);
    a.heard(Some(Song { year: 2020, suffix: "flac".into(), ..song("s1", "First", 200) }));
    a.now.state = State::Playing;
    a.now.mixing = true;
    a.transition = Some(nori_core::automix::planner::TransitionNote {
        outgoing_id: "s1".into(),
        incoming_id: "s2".into(),
        kind: "BeatMatched".into(),
        start_ms: 190_000,
        duration_ms: 8_000,
        tempo_ratio: 1.02,
        reason: "bars aligned".into(),
    });
    a.go(Screen::Playing);
    let s = draw(&mut a, 120, 30);
    assert!(s.contains("First") && s.contains("mixing into the next") && s.contains("beat-matched") && s.contains("Next: “Second”"), "{s}");
    dump("playing", &s);
}

#[test]
fn lyrics_follow_the_song_word_by_word() {
    use nori_core::{LyricLine, LyricWord, Lyrics};
    let mut a = app();
    a.heard(Some(song("s1", "First", 200)));
    a.go(Screen::Lyrics);
    assert!(a.cmds.contains(&Cmd::Lyrics("s1".into())));
    let words = vec![LyricWord { start_ms: 1000, end_ms: 1500, start: 0, end: 5 }, LyricWord { start_ms: 1500, end_ms: 2000, start: 6, end: 11 }];
    let lines = vec![
        LyricLine { start_ms: 1000, end_ms: 2000, text: "Hello world".into(), words, ..Default::default() },
        LyricLine { start_ms: 3000, end_ms: 4000, text: "Second line".into(), ..Default::default() },
    ];
    let pick = nori_core::race::LyricsPick { lyrics: Lyrics { synced: true, word_timed: true, lines, key: 0 }, origin: nori_core::lyrics_sources::LyricsOrigin::Server };
    a.handle(Msg::Lyrics { song: "s1".into(), pick });
    let l = a.lyrics.as_ref().unwrap();
    l.advance(1250, true, true);
    let s = draw(&mut a, 80, 20);
    assert!(s.contains("Hello world") && s.contains("Second line"), "{s}");
    dump("lyrics", &s);
    // Enter on a line seeks to it.
    key(&mut a, KeyCode::Down);
    key(&mut a, KeyCode::Enter);
    assert_eq!(a.cmds.last(), Some(&Cmd::Seek(3000)));
}

#[test]
fn the_equalizer_draws_its_bands_as_bars() {
    let mut a = app();
    a.prefs.eq_enabled = true;
    a.prefs.eq_bands = nori_core::settings::graphic();
    a.prefs.eq_bands[2].gain_db = 12.0;
    a.prefs.eq_bands[5].gain_db = -6.0;
    a.go(Screen::Equalizer);
    assert!(!a.cmds.iter().any(|c| matches!(c, Cmd::Tuning(_))), "opening the screen leaves the output as it is");
    let s = draw(&mut a, 100, 36);
    assert!(s.contains("█") && s.contains("1k") && s.contains("Presets"), "{s}");
    dump("equalizer", &s);
    // Moving around the screen and redrawing it asks nothing of the settings or the engine.
    let before = a.cmds.len();
    for _ in 0..5 {
        key(&mut a, KeyCode::Down);
        draw(&mut a, 100, 36);
    }
    key(&mut a, KeyCode::Up);
    assert_eq!(a.cmds.len(), before, "{:?}", &a.cmds[before..]);
    // The first sound really changed there asks for the shallow buffer, once.
    assert!(a.sound_edited());
    assert!(!a.sound_edited());
    a.go(Screen::Home);
    assert!(a.cmds.contains(&Cmd::Tuning(false)));
    // Left without changing anything: nothing to ask back.
    a.cmds.clear();
    a.go(Screen::Equalizer);
    a.go(Screen::Home);
    assert!(!a.cmds.iter().any(|c| matches!(c, Cmd::Tuning(_))), "{:?}", a.cmds);
    // With the equalizer off nothing changed on it is heard: the output is left alone.
    a.prefs.eq_enabled = false;
    a.go(Screen::Equalizer);
    assert!(!a.sound_edited());
    // Switched off while the shallow buffer was held: given back.
    a.prefs.eq_enabled = true;
    assert!(a.sound_edited());
    a.cmds.clear();
    let off = StoredPrefs { eq_enabled: false, ..a.prefs.clone() };
    a.prefs_changed(off);
    assert_eq!(a.cmds, [Cmd::Tuning(false)]);
}

#[test]
fn a_picture_in_a_graphics_protocol_does_not_hide_the_rest_of_the_frame() {
    use ratatui_image::picker::{Picker, ProtocolType};
    for protocol in [ProtocolType::Sixel, ProtocolType::Kitty, ProtocolType::Iterm2, ProtocolType::Halfblocks] {
        let mut a = app();
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(protocol);
        let mut art = crate::art::Art::new(picker);
        let px = vec![120u8; 64 * 64 * 4].into_boxed_slice();
        art.put("c1".into(), &std::sync::Arc::new(nori_covers::memory::Image { width: 64, height: 64, pixels: px }));
        a.heard(Some(Song { cover_art: Some("c1".into()), ..song("s1", "First", 200) }));
        a.go(Screen::Playing);
        // The TestBackend is written through ratatui's diff, as a terminal is: whatever the diff skips is
        // missing from it.
        let s = draw_with(&mut a, 120, 30, Some(&mut art));
        assert!(s.contains("Up next") && s.contains("3:20") && s.contains("q quit"), "{protocol:?} hid the frame:\n{s}");
    }
}

#[test]
fn focus_lost_draws_nothing_and_focus_back_draws() {
    let mut a = app();
    a.dirty = false;
    a.handle(Msg::Focus(false));
    assert!(!a.dirty, "nothing changed on screen");
    a.handle(Msg::Focus(true));
    assert!(a.dirty, "back in view: drawn again");
}

/// The player bar's play/pause button, as drawn (the controls' middle symbol).
fn button(s: &str) -> &'static str {
    let bar = s.lines().rev().find(|l| l.contains('⏮')).expect("the controls are drawn");
    if bar.contains('⏸') {
        "pause"
    } else if bar.contains('▶') {
        "play"
    } else {
        panic!("no play or pause button: {bar}")
    }
}

#[test]
fn space_and_the_engines_answer_change_the_play_button_at_once() {
    let mut t = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let mut frame = |a: &mut App| {
        t.draw(|f| crate::ui::draw(f, a, None)).unwrap();
        a.dirty = false;
        let buf = t.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n").collect::<String>()
    };
    let mut a = app();
    a.heard(Some(song("s1", "First", 200)));
    a.now.state = State::Paused;
    assert_eq!(button(&frame(&mut a)), "play");
    // Space asks the engine, and is drawn.
    key(&mut a, KeyCode::Char(' '));
    assert!(a.cmds.contains(&Cmd::Toggle), "{:?}", a.cmds);
    assert!(a.dirty);
    // The engine's answer alone, with no key pressed, redraws the button.
    a.handle(Msg::Engine(nori_engine::Event::State(State::Playing)));
    assert!(a.dirty, "an engine event is drawn");
    assert_eq!(button(&frame(&mut a)), "pause");
    a.handle(Msg::Engine(nori_engine::Event::State(State::Paused)));
    assert!(a.dirty);
    assert_eq!(button(&frame(&mut a)), "play");
}

#[test]
fn a_message_that_changes_nothing_does_not_undo_the_draw_an_event_before_it_asked_for() {
    let mut a = app();
    a.dirty = false;
    // Taken in one batch: the engine paused, then the terminal lost focus (which alone draws nothing).
    a.handle(Msg::Engine(nori_engine::Event::State(State::Paused)));
    a.handle(Msg::Focus(false));
    assert!(a.dirty, "the pause is still to be drawn");
}

#[test]
fn the_clock_stops_where_it_was_when_the_engine_says_paused() {
    let mut a = app();
    let start = Instant::now() - Duration::from_secs(5);
    a.now = crate::app::Now { state: State::Playing, position_ms: 10_000, at: start, ..Default::default() };
    a.handle(Msg::Engine(nori_engine::Event::State(State::Paused)));
    let at = a.now.position(Instant::now());
    assert!((14_900..16_000).contains(&at), "stopped about 15 s in, not back at 10 s: {at}");
    assert_eq!(a.now.position(Instant::now() + Duration::from_secs(60)), at, "paused: the clock stands, a minute on too");
}

#[test]
fn the_engines_status_is_not_believed_over_its_newer_events() {
    // The engine says a change before its status shows it: read in between, the status is behind.
    let mut said = crate::runner::Said::default();
    assert_eq!(said.behind(State::Idle, None), None, "no event yet: the status stands");
    said.state = Some(State::Playing);
    assert_eq!(said.behind(State::Idle, Some("a")), Some(None), "the status still says idle");
    assert_eq!(said.behind(State::Playing, Some("a")), None, "caught up");
    said.song = Some("b".into());
    assert_eq!(said.behind(State::Playing, Some("a")), Some(Some("b".into())), "the song the event named is shown");
    assert_eq!(said.behind(State::Playing, Some("b")), None);
}

#[test]
fn tmux_is_trusted_with_sixel_only_on_a_terminal_it_found_draws_sixel() {
    use crate::term::sixel_feature;
    // Ghostty (kitty graphics, no sixel) under tmux 3.7: tmux still answers that it draws sixel, and
    // would show a box of + signs; the pictures go through to Ghostty instead.
    assert!(!sixel_feature("bpaste,ccolour,clipboard,cstyle,focus,RGB,title"));
    assert!(!sixel_feature(""));
    // xterm as a VT340, foot, WezTerm with sixel: tmux keeps and draws the picture.
    assert!(sixel_feature("256,bpaste,ccolour,clipboard,cstyle,extkeys,focus,mouse,rectfill,RGB,sixel,strikethrough,title"));
}

#[test]
fn a_cover_is_sent_on_the_first_frame_and_again_when_the_song_changes() {
    use ratatui_image::picker::{Picker, ProtocolType};
    let image = |v: u8| std::sync::Arc::new(nori_covers::memory::Image { width: 64, height: 64, pixels: vec![v; 64 * 64 * 4].into_boxed_slice() });
    for protocol in [ProtocolType::Sixel, ProtocolType::Kitty, ProtocolType::Iterm2] {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(protocol);
        let mut art = crate::art::Art::new(picker);
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut a = app();
        a.go(Screen::Playing);
        a.heard(Some(Song { cover_art: Some("c1".into()), ..song("s1", "First", 200) }));
        assert!(a.cmds.contains(&Cmd::Cover { art: "c1".into(), colours: true }), "the heard song's cover is asked for");
        // What the terminal is sent in a frame: ratatui's diff, as written to it.
        fn sent(t: &mut Terminal<TestBackend>, a: &mut App, art: &mut crate::art::Art) -> usize {
            let before = t.backend().buffer().clone();
            t.draw(|f| crate::ui::draw(f, a, Some(art))).unwrap();
            let after = t.backend().buffer();
            before.diff(after).into_iter().filter(|(_, _, c)| c.symbol().starts_with('\x1b')).count()
        }
        art.put("c1".into(), &image(100));
        assert!(sent(&mut t, &mut a, &mut art) > 0, "{protocol:?}: the first cover is sent");
        // (kitty's next frame swaps the transmission for a bare placement: one cell, once)
        sent(&mut t, &mut a, &mut art);
        assert_eq!(sent(&mut t, &mut a, &mut art), 0, "{protocol:?}: and not again while it stays");
        a.heard(Some(Song { cover_art: Some("c2".into()), ..song("s2", "Second", 200) }));
        art.put("c2".into(), &image(200));
        assert!(sent(&mut t, &mut a, &mut art) > 0, "{protocol:?}: the next song's cover is sent");
        // A pane back from another tmux window: every picture made again and the whole screen written
        // (term::repaint: a blank frame, then the next draw writes everything).
        art.resend();
        t.draw(|_| {}).unwrap();
        assert!(sent(&mut t, &mut a, &mut art) > 0, "{protocol:?}: sent again after resend");
    }
}

#[test]
fn a_queue_started_from_a_page_carries_the_page() {
    use nori_core::{OriginKind, PageOrigin, Playlist, PlaylistDetail};
    let mut a = app();
    a.go(Screen::Library);
    a.open_album("al-3".into());
    let songs = vec![song("1", "One", 200), song("2", "Two", 200)];
    a.handle(Msg::Data(Req::Album("al-3".into()), Ok(Data::Album(Box::new(nori_core::AlbumDetail::new(album("al-3", "Three"), songs.clone(), vec![]))))));
    // A row tapped plays the album's songs from it, as the album's own queue; so does the page's Play.
    a.cmds.clear();
    key(&mut a, KeyCode::Down);
    key(&mut a, KeyCode::Enter);
    key(&mut a, KeyCode::Char('x'));
    let from = |c: &Cmd| match c {
        Cmd::Play { from, start, .. } => Some((from.clone(), *start)),
        _ => None,
    };
    let album_page = Some(PageOrigin::new(OriginKind::Album, "al-3"));
    assert_eq!(a.cmds.iter().filter_map(from).collect::<Vec<_>>(), [(album_page.clone(), 1), (album_page, 0)]);
    // A playlist's page is its own, not the album's its songs come from.
    key(&mut a, KeyCode::Esc);
    a.open_playlist("pl-1".into());
    let pl = PlaylistDetail::new(Playlist { id: "pl-1".into(), ..Default::default() }, songs);
    a.handle(Msg::Data(Req::Playlist("pl-1".into()), Ok(Data::Playlist(Box::new(pl)))));
    a.cmds.clear();
    key(&mut a, KeyCode::Enter);
    assert_eq!(a.cmds.iter().filter_map(from).collect::<Vec<_>>(), [(Some(PageOrigin::new(OriginKind::Playlist, "pl-1")), 0)]);
}
