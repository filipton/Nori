# JNA + uniffi
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class dev.nori.music.ffi.** { *; }
-dontwarn java.awt.**

# The transition engine reaches these by name from native code (crates/core/src/automix/engine_jni.rs):
# the sink's down* callbacks and hostHeardChanged.
-keep class dev.nori.music.playback.TransitionSink { *; }
-keep class dev.nori.music.playback.TransitionEngineJni { native <methods>; }
