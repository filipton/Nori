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

internal object AutoMixMixer {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun create(sampleRate: Int, channels: Int): Long
    /** [params] from `automixMixerParams(plan)`; restarts the transition clock. */
    @JvmStatic external fun configure(h: Long, params: FloatArray)
    /** Mixes [frames] frames of each stream into [dest] (which may be either input). */
    @JvmStatic external fun process(h: Long, outgoing: ByteBuffer, outPos: Int, incoming: ByteBuffer, inPos: Int, dest: ByteBuffer, destPos: Int, frames: Int, encoding: Int): Boolean
    @JvmStatic external fun position(h: Long): Long
    /** Starts the transition clock [frames] in: curves and filters as if the mix had run that far. */
    @JvmStatic external fun seek(h: Long, frames: Long)
    @JvmStatic external fun destroy(h: Long)
}

/** Converts between sample rates (and mono/stereo): crates/core/src/automix/resample.rs. Every buffer argument is a direct ByteBuffer; sizes are in bytes. */
internal object AutoMixResample {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun create(inRate: Int, inChannels: Int, outRate: Int, outChannels: Int): Long
    /** Returns (consumed shl 32) or produced, both in bytes; -1 when the buffers cannot be used. */
    @JvmStatic external fun process(h: Long, input: ByteBuffer, inPos: Int, inBytes: Int, output: ByteBuffer, outPos: Int, outCap: Int, inEnc: Int, outEnc: Int): Long
    @JvmStatic external fun destroy(h: Long)
}

internal object AutoMixStretch {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun create(sampleRate: Int, channels: Int, keepPitch: Boolean): Long
    /** [ratio] is playback speed, held for [holdFrames] output frames, then ramped to 1 over [rampFrames]. */
    @JvmStatic external fun configure(h: Long, ratio: Float, holdFrames: Long, rampFrames: Long)
    /** Returns (consumed shl 32) or produced, both in bytes; -1 when the buffers cannot be used. */
    @JvmStatic external fun process(h: Long, input: ByteBuffer, inPos: Int, inBytes: Int, output: ByteBuffer, outPos: Int, outCap: Int, encoding: Int): Long
    @JvmStatic external fun drain(h: Long, output: ByteBuffer, outPos: Int, outCap: Int, encoding: Int): Int
    @JvmStatic external fun bypassed(h: Long): Boolean
    @JvmStatic external fun latencyFrames(h: Long): Int
    @JvmStatic external fun destroy(h: Long)
}
