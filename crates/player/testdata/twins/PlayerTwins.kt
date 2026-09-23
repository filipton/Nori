// The Kotlin originals of nori-player's twins, run on the host JVM by tools/twins.sh to write
// player_twins.tsv, which crates/player/tests/twins.rs holds the Rust to. Each function is the app's own
// code, copied as it is; media3's and Android's objects in it are written as the plain values the code
// reads from them (a MediaFormat's keys as nullable numbers, a Format's initializationData as a list).
// Copy a function again when its original changes, and run tools/twins.sh.

fun row(vararg f: Any?) = println(f.joinToString("\t"))

fun hex(b: ByteArray?): String = when {
    b == null -> "-"
    b.isEmpty() -> "_"
    else -> b.joinToString("") { "%02x".format(it) }
}

// ---- core/.../playback/AutoMixPrefetch.kt: setupData, over csd-0, csd-1, ... ----

fun setupDataExtracted(parts: List<ByteArray>): ByteArray? {
    if (parts.isEmpty()) return null
    val all = ByteArray(parts.sumOf { it.size })
    var at = 0
    for (p in parts) { val n = p.size; p.copyInto(all, at); at += n }
    return all
}

// ---- core/.../playback/RustAudio.kt: RustAudioDecoder.setupData, over initializationData ----

fun setupDataFormat(parts: List<ByteArray>): ByteArray? {
    if (parts.isEmpty()) return null
    if (parts.size == 1) return parts[0]
    return ByteArray(parts.sumOf { it.size }).also { all -> var at = 0; for (p in parts) { p.copyInto(all, at); at += p.size } }
}

// ---- core/.../playback/AutoMixPrefetch.kt inCore: the codec string and the packet buffer ----

fun codecs(aacProfile: Int?): String? = if (aacProfile != null) "mp4a.40." + aacProfile else null

const val DEFAULT_INPUT = 64 * 1024
const val MIN_INPUT = 8 * 1024
fun measuringBuffer(maxInputSize: Int?): Int = if (maxInputSize != null) maxInputSize.coerceAtLeast(MIN_INPUT) else DEFAULT_INPUT
fun grown(capacity: Int, sampleSize: Long): Int = if (sampleSize > capacity) sampleSize.toInt() else capacity

// ---- core/.../playback/RustAudio.kt RustAudioDecoder: the input buffer (Format.NO_VALUE is -1) ----

const val DEFAULT_INPUT_SIZE = 64 * 1024
fun decoderBuffer(maxInputSize: Int): Int = if (maxInputSize != -1) maxInputSize else DEFAULT_INPUT_SIZE

// ---- core/.../playback/TransitionSink.kt configure: the encoding, and the formats kept by token ----

const val AUDIO_RAW = "audio/raw"
fun encoding(mime: String?, pcmEncoding: Int): Int = if (mime == AUDIO_RAW && (pcmEncoding == 2 || pcmEncoding == 4)) pcmEncoding else 0

class Configs {
    val configs = HashMap<Int, Any>()
    var nextToken = 0
    fun configure() {
        val token = nextToken++
        configs[token] = Any()
        if (configs.size > 16) configs.keys.filter { it < token - 16 }.forEach(configs::remove)
    }
    fun reset() = configs.clear()
}

fun main() {
    val parts = listOf(
        emptyList(),
        listOf(byteArrayOf()),
        listOf(byteArrayOf(0x12, 0x10)),
        listOf(byteArrayOf(1, 2, 3), byteArrayOf(4), byteArrayOf(), byteArrayOf(5, 6)),
        listOf(byteArrayOf(), byteArrayOf()),
    )
    for (p in parts) {
        val given = if (p.isEmpty()) "-" else p.joinToString(",") { hex(it) }
        row("setup_extracted", given, hex(setupDataExtracted(p)))
        row("setup_format", given, hex(setupDataFormat(p)))
    }

    for (profile in listOf(null, 2, 5, 29, 0, -1)) row("codecs", profile ?: "-", codecs(profile) ?: "-")

    for (max in listOf(null, -1, 0, 1, 4_096, 8_191, 8_192, 8_193, 65_536, 1_000_000)) row("measuring_buffer", max ?: "-", measuringBuffer(max))
    for (max in listOf(-1, 0, 1, 4_096, 65_536, 1_000_000)) row("decoder_buffer", max, decoderBuffer(max))
    for (cap in listOf(8_192, 65_536)) for (size in listOf(-1L, 0L, 8_192L, 8_193L, 65_536L, 70_000L)) row("grown", cap, size, grown(cap, size))

    for (mime in listOf(AUDIO_RAW, "audio/mpeg", "audio/flac", null)) for (enc in listOf(-1, 0, 1, 2, 3, 4, 21, 22, 268435456, 536870912)) {
        row("encoding", mime ?: "-", enc, encoding(mime, enc))
    }

    val c = Configs()
    for (step in 0 until 60) {
        if (step == 40) c.reset() else c.configure()
        row("configs", step, c.configs.keys.sorted().joinToString(",").ifEmpty { "_" })
    }
}
