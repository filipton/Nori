package dev.nori.music.playback

import java.nio.ByteBuffer

/** The Rust side of AutoMix: crates/core/src/automix. Every buffer argument is a direct ByteBuffer; sizes are in bytes. */
internal object AutoMixAnalyzer {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun create(sampleRate: Int, channels: Int, expectedMs: Long): Long
    /** Reads only. Downmixes to mono and folds the audio into the running analysis. */
    @JvmStatic external fun feed(h: Long, buffer: ByteBuffer, pos: Int, bytes: Int, encoding: Int): Boolean
    @JvmStatic external fun frames(h: Long): Long
    @JvmStatic external fun reset(h: Long)
    @JvmStatic external fun destroy(h: Long)
}

/**
 * The transition engine (`nori_player::engine`, through `crates/core/src/automix/engine_jni.rs`). Every
 * call that can reach the output below takes the [TransitionSink] it calls back into, and the time now
 * (elapsedRealtime), which the engine stamps what it reports with.
 */
internal object TransitionEngineJni {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun create(): Long
    @JvmStatic external fun destroy(h: Long)
    @JvmStatic external fun replan(h: Long)
    @JvmStatic external fun setLockRate(h: Long, on: Boolean)
    @JvmStatic external fun setOffset(h: Long, offsetUs: Long)
    /** [encoding] is media3's; anything but 16-bit or float PCM is not samples. */
    @JvmStatic external fun configure(h: Long, sink: TransitionSink, nowMs: Long, id: String?, rate: Int, channels: Int, encoding: Int, token: Int)
    /** `(taken << 32) | bytes used` of [len] bytes of [buffer] from [pos]. */
    @JvmStatic external fun handleBuffer(h: Long, sink: TransitionSink, nowMs: Long, buffer: ByteBuffer, pos: Int, len: Int, ptsUs: Long, downPositionUs: Long): Long
    @JvmStatic external fun handleDiscontinuity(h: Long, sink: TransitionSink, nowMs: Long)
    /** [downPositionUs] is the output's own position, read just before: the engine's first question about it. */
    @JvmStatic external fun position(h: Long, sink: TransitionSink, nowMs: Long, sourceEnded: Boolean, downPositionUs: Long): Long
    @JvmStatic external fun playToEnd(h: Long, sink: TransitionSink, nowMs: Long): Boolean
    /** A direct buffer over the engine's status words, read with no call: long 0 is whether audio is queued, long 1 the bytes handed to the output. */
    @JvmStatic external fun status(h: Long): java.nio.ByteBuffer
    @JvmStatic external fun mixing(): Boolean
    /** Bursts on or off (see nori_player::burst). */
    @JvmStatic external fun setBurst(h: Long, on: Boolean)
    @JvmStatic external fun restartBurst(h: Long)
    @JvmStatic external fun holding(): Boolean
    @JvmStatic external fun queueEmpty(h: Long, sink: TransitionSink, nowMs: Long): Boolean
    @JvmStatic external fun flush(h: Long, sink: TransitionSink, nowMs: Long)
    @JvmStatic external fun reset(h: Long, sink: TransitionSink, nowMs: Long)
}

/** Which song the ear is on during a mix; see crates/core/src/heard.rs. */
internal object HeardJni {
    init { System.loadLibrary("norimusic") }

    @JvmStatic external fun create(): Long
    @JvmStatic external fun destroy(h: Long)
    /** `(index + 1) << 44 | changed << 43 | ms`; index -1 means the player's own word stands. */
    @JvmStatic external fun at(h: Long, nowMs: Long, playing: Boolean, on: Int, next: Int, positionMs: Long): Long
}

/** Making a seek stick; see crates/core/src/seek.rs. */
internal object SeekJni {
    init { System.loadLibrary("norimusic") }

    @JvmStatic external fun create(): Long
    @JvmStatic external fun ask(h: Long, target: Long, now: Long, ready: Boolean, pos: Long)
    @JvmStatic external fun forget(h: Long)
    /** -1 keep watching, -2 done with it, otherwise the place to ask the player for again. */
    @JvmStatic external fun look(h: Long, now: Long, sameSong: Boolean, ready: Boolean, pos: Long, playing: Boolean): Long
}
