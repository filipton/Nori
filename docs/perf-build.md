# The perf build

A release build with a recorder in it, for measuring what the app costs on a real phone in real use:
battery, CPU, wakeups, allocations, memory and frames, with no adb and no computer attached. The
results are on a Performance page in settings, and a plain-text report can be shared from there.

It is minified and not debuggable, like a release, so what it measures is what a release costs. It
installs beside the normal app (id `dev.nori.music.perf`, named "Nori perf"), with its own sign-in,
settings and downloads.

## Building and installing

```sh
./gradlew :app:assemblePerf -PrustTargets=arm64-v8a            # a phone
./gradlew :app:assemblePerf -PrustTargets=x86_64               # the emulator
```

The APK is `app/build/outputs/apk/perf/app-perf.apk`, signed with the Android debug key. Put it on the
phone either way:

- `adb install -r app/build/outputs/apk/perf/app-perf.apk`, or
- send the file to the phone (a chat to yourself, a cloud drive, a USB cable) and open it there; Android
  asks once to allow installing from that app.

Open Nori perf, sign in, and the recorder is already running: it starts with the app's process.
Settings > Performance is the page.

## What it records

The app's life is cut into **stretches**, each spent in one state:

| State | |
|---|---|
| Screen off, playing | the number that matters most: music in the pocket |
| Screen off, paused | should be close to nothing |
| Screen on, playing, player open | the player sheet up |
| Screen on, playing, other page | the app on screen, anywhere but the player |
| Screen on, playing, another app | the phone in use, the music in the background |
| Screen on, paused | |
| Charging | kept apart; left out of every battery figure |

A stretch also ends when a setting that changes the cost changes (equalizer, AutoMix, crossfade,
offload, hi-res, bit-perfect), so every stretch was spent with one set of them; the stretch lists them,
and whether the music was offloaded to the audio chip or played on the CPU when it ended.

For each stretch:

| Figure | What it is |
|---|---|
| time | how long the stretch lasted (the elapsed-realtime clock, which counts deep sleep) |
| CPU % | the app process's CPU time (utime + stime, `/proc/self/stat`) over the stretch, in % of one core |
| wakeups/s | voluntary context switches of the app's threads per second (`/proc/self/task/*/status`): each is a thread going to sleep and being woken again, which is what keeps the CPU out of deep idle. Summed over the threads alive at both ends, as `tools/bench.sh` does |
| KB/min | Java heap allocated per minute (`art.gc.bytes-allocated`); GCs is the collections run |
| PSS MB | the process's proportional memory at the end of the stretch (`Debug.MemoryInfo`) |
| mAh, mAh/h | charge used by the whole phone, from the battery's own counter (`BATTERY_PROPERTY_CHARGE_COUNTER`); mAh/h is the average current in mA. Phones without the counter show the battery level's drop in % and %/h instead |
| gauge mA | what the fuel gauge said at the two ends (its own average, `CURRENT_AVERAGE`, where it keeps one, else `CURRENT_NOW`): a cross-check, not a measurement |
| °C | battery temperature at the two ends |
| frames, janky | frames the app drew while on screen, and those that missed their deadline (Android 12 on) or took longer than one refresh of the display (before); the slowest frame's time |
| network | bytes the app received and sent over the stretch (`TrafficStats`), where the phone counts them |

Under each stretch, on the page and in the report, two lines say why it cost what it did:

| Line | What it is |
|---|---|
| threads by wakeups | the six threads that woke most over the stretch (`/proc/self/task/*/stat` and `status`): each one's name, wakeups per second and CPU time. A thread that started during the stretch is marked "(new)". The Rust player's threads carry their own names (`nori-engine`, `nori-track`, `nori-load`, `nori-open`, `nori-covers`, `nori-analysis`); Java's are the platform's (`binder:…`, `RenderThread`, `main` is the app's package name) |
| output | the AudioTrack the player last opened, as the platform describes it when the stretch ended, against what was asked of it: the engine, rate, channels and encoding, the buffer in ms (given, of what was asked, and the most it could grow to), the performance mode asked for and given (the Rust player asks for power saving, the platform's deep-buffer output; media3 asks for none), whether it is offloaded to the audio chip, where it is routed, its underruns since it was made, and whether it was playing |

A buffer much smaller than asked, or power saving asked and not given, is what makes a writer wake more
often than the design says: the Rust player's writer then tops the track up once per half of what it
holds (and says so in the log), which the thread line shows as `nori-track`'s wakeups.

"By state" adds the stretches of each state up. Battery is the whole phone's, not the app's alone:
the screen, the radio and every other app are in it, which is why a fair test holds them still (below).
CPU, wakeups, allocations and PSS are the app's own.

Stretches are kept in the app's database (`perf_stretches`, crates/core/src/perf_log.rs) for 14 days.
Stretches under 3 seconds are the blinks between two states (the screen going off also stops the
activity) and are dropped. Android reads the counters and counts the frames (`app/src/perf`); which
state a stretch is filed under, what two readings make, the sums by state and every word on the page
and in the report are the core's (`perf_state`, `perf_stretch`, `perf_page`, `perf_report`), so a
desktop recorder files and reports the same way. The stretch under way when the process dies is lost; the page shows it as
"Now" while it lasts.

## What it costs

Nothing ticks. The counters are read only when something happens: the screen goes on or off, the
power is connected or not, playback starts or stops (the service's own broadcast), the app comes to
the front or leaves it, the player opens or closes, a setting above changes, or the Performance page
opens. Each reading takes a few milliseconds (the PSS is most of it), on a thread of its own that
sleeps in its looper in between. With the screen off and music playing the recorder does not run at
all, so it adds **no wakeups of its own** to the numbers it records. The thread list, the output and the
network bytes are read at the same moments, not in between.

While the app is on screen, Android hands every frame's timings to that thread
(`Window.addOnFrameMetricsAvailableListener`); the listener is removed when the app leaves the
screen. That is a small cost per frame drawn, in the screen-on stretches only.

The benchmark buttons are real work: their cost lands in the stretch under way.

## A fair battery test

Battery figures need long stretches: many phones move the charge counter in steps of a few mAh, and
the level in whole percents. For one setting or one build against another:

1. Unplugged, and not charged during the run (a charging stretch is not counted).
2. The same playlist, the same volume, the same output (speaker, the same headphones), the same
   network (Wi-Fi or mobile, the same place), downloaded or streamed the same way.
3. Screen off, 1 to 2 hours per stretch. Don't touch the phone: waking the screen ends the stretch.
4. Three runs of each, and compare the mAh/h of "Screen off, playing". One run can be off by a lot:
   the phone's own background work comes and goes.
5. The normal Nori app should not be playing at the same time.

Press "Start fresh" before a series so the table holds only that series, and share the report after
each (the report lists every stretch, with its settings).

## Sharing

"Share report" opens Android's share sheet with the report as plain text: the device, the Android
version, the build (version and commit), the table by state, the last stretches, and the benchmark
results if they were run this session. Send it anywhere text goes; the table is in columns for a
monospaced font.

## Benchmarks

"Run call benchmark" and "Run cover benchmark" run the same code as the debug build's `bench` and
`coverbench` test commands (`app/src/bench/.../Bench.kt`): what a crossing into the core costs by kind
against the same work in Kotlin, and what the core's covers cost: the decode alone, and the whole way to a
software or hardware Bitmap (docs/clients.md). The cover benchmark needs covers on the disk and in memory,
so browse some albums first.
