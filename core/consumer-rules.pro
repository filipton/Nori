# uniffi's generated Kotlin. The native side finds package uniffi by class, method and JVM signature
# (the Scaffolding natives by their Java_ names; callbacks, lifted results and coroutine wake-ups by
# GetStaticMethodID), so nothing there, nor any type in those signatures, may be renamed or dropped.
-keep,includedescriptorclasses class uniffi.** { *; }
-keep class dev.nori.music.ffi.** { *; }

# The transition engine reaches these by name from native code (crates/android/src/engine.rs):
# the sink's down* callbacks and hostHeardChanged.
-keep class dev.nori.music.playback.TransitionSink { *; }

# The native library registers every JNI door by class and method name when it loads
# (crates/android/src/lib.rs), so neither may be renamed or dropped.
-keepclasseswithmembers class dev.nori.music.** { native <methods>; }
