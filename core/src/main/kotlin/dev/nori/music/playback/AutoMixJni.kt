package dev.nori.music.playback

import dalvik.annotation.optimization.CriticalNative
import dalvik.annotation.optimization.FastNative
import java.nio.ByteBuffer

/** The Rust side of AutoMix: crates/automix. Every buffer argument is a direct ByteBuffer; sizes are in bytes. */
internal object AutoMixAnalyzer {
    init { System.loadLibrary("norimusic") }
    @JvmStatic @CriticalNative external fun create(sampleRate: Int, channels: Int, expectedMs: Long): Long
    /** Reads only. Downmixes to mono and folds the audio into the running analysis. */
    @JvmStatic @FastNative external fun feed(h: Long, buffer: ByteBuffer, pos: Int, bytes: Int, encoding: Int): Boolean
    /**
     * Decodes one packet ([len] bytes at [pos] of [input]) with the core's decoder [d] ([RustDecoderJni.create])
     * and folds it in, the samples never crossing back. Frames heard; -1 skip the packet; -2 the core cannot go on.
     */
    @JvmStatic @FastNative external fun decode(h: Long, d: Long, input: ByteBuffer, pos: Int, len: Int): Int
    @JvmStatic @CriticalNative external fun frames(h: Long): Long
    @JvmStatic @CriticalNative external fun reset(h: Long)
    @JvmStatic @CriticalNative external fun destroy(h: Long)
}

/**
 * The transition engine (`nori_player::engine`, through `crates/android/src/engine.rs`). Every
 * call that can reach the output below takes the [TransitionSink] it calls back into, and the time now
 * (elapsedRealtime), which the engine stamps what it reports with.
 */
internal object TransitionEngineJni {
    init { System.loadLibrary("norimusic") }
    @JvmStatic @CriticalNative external fun create(): Long
    @JvmStatic @CriticalNative external fun destroy(h: Long)
    @JvmStatic @CriticalNative external fun replan(h: Long)
    @JvmStatic @CriticalNative external fun setLockRate(h: Long, on: Boolean)
    @JvmStatic @CriticalNative external fun setOffset(h: Long, offsetUs: Long)
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
    /** Bytes the last engine handed to the output; needs no handle, so it is safe after a sink is freed. */
    @JvmStatic @CriticalNative external fun bytesWritten(): Long
    @JvmStatic @CriticalNative external fun mixing(): Boolean
    /** Bursts on or off (see nori_player::burst). */
    @JvmStatic @CriticalNative external fun setBurst(h: Long, on: Boolean)
    @JvmStatic @CriticalNative external fun restartBurst(h: Long)
    @JvmStatic @CriticalNative external fun holding(): Boolean
    @JvmStatic external fun queueEmpty(h: Long, sink: TransitionSink, nowMs: Long): Boolean
    @JvmStatic external fun flush(h: Long, sink: TransitionSink, nowMs: Long)
    @JvmStatic external fun reset(h: Long, sink: TransitionSink, nowMs: Long)
}

/** Which song the ear is on during a mix; see crates/queue/src/heard.rs. */
internal object HeardJni {
    init { System.loadLibrary("norimusic") }

    @JvmStatic @CriticalNative external fun create(): Long
    @JvmStatic @CriticalNative external fun destroy(h: Long)
    /** `(index + 1) << 44 | changed << 43 | ms`; index -1 means the player's own word stands. */
    @JvmStatic @CriticalNative external fun at(h: Long, nowMs: Long, playing: Boolean, on: Int, next: Int, positionMs: Long): Long
}

/** Making a seek stick; see crates/android/src/seek.rs. */
internal object SeekJni {
    init { System.loadLibrary("norimusic") }

    @JvmStatic @CriticalNative external fun create(): Long
    @JvmStatic @CriticalNative external fun ask(h: Long, target: Long, now: Long, ready: Boolean, pos: Long)
    @JvmStatic @CriticalNative external fun forget(h: Long)
    /** -1 keep watching, -2 done with it, otherwise the place to ask the player for again. */
    @JvmStatic @CriticalNative external fun look(h: Long, now: Long, sameSong: Boolean, ready: Boolean, pos: Long, playing: Boolean): Long
}
