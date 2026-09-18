package dev.flint.music.data

import dev.flint.music.ffi.Lyrics
import dev.flint.music.ffi.Song
import dev.flint.music.ffi.lyricsFromLrc
import dev.flint.music.net.Http
import org.json.JSONArray
import org.json.JSONObject
import java.net.URLEncoder

/** Where a set of lyrics came from, for the credit line under them. */
enum class LyricsSource(val label: String) { SERVER("your server"), LRCLIB("LRCLIB") }

data class FoundLyrics(val lyrics: Lyrics, val source: LyricsSource)

/** A provider's answer. [Failed] (no network, server error) must never be remembered as [Missing]. */
sealed interface Lookup {
    data class Found(val lyrics: Lyrics) : Lookup
    data object Missing : Lookup
    data object Failed : Lookup
}

/**
 * Lyrics from outside the server, for songs it has none (or only unsynced ones) for. LRCLIB is an open,
 * community-run database of synced lyrics with a documented API, which is why it is the one built in.
 * Each lookup is one or two small requests made when the lyrics are opened, never ahead of time.
 */
class LyricsProviders(private val http: () -> Http) {
    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8")

    /** Removes "(feat. X)", "- Remastered 2011" and similar, which providers rarely carry. */
    private fun clean(title: String) = title
        .replace(Regex("""\s*[(\[](feat\.?|ft\.?|with|remaster(ed)?|\d{4} remaster|live|mono|stereo)[^)\]]*[)\]]""", RegexOption.IGNORE_CASE), "")
        .replace(Regex("""\s+-\s+(remaster(ed)?|\d{4} remaster|live|single version|radio edit).*$""", RegexOption.IGNORE_CASE), "")
        .trim()

    suspend fun lrclib(song: Song): Lookup {
        val title = clean(song.title)
        val base = "https://lrclib.net/api"
        // The exact lookup first: LRCLIB matches the duration within a couple of seconds itself.
        val exact = try {
            JSONObject(http().get("$base/get?artist_name=${enc(song.artist)}&track_name=${enc(title)}&album_name=${enc(song.album)}&duration=${song.duration}").decodeToString())
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            android.util.Log.w("flint", "lrclib get failed: $e")
            return Lookup.Failed
        }
        if (!exact.has("statusCode")) pick(exact)?.let { return Lookup.Found(it) }
        // Otherwise a search, ranked by how close the duration is: the first hit is often a remix or a live take.
        val hits = try {
            JSONArray(http().get("$base/search?track_name=${enc(title)}&artist_name=${enc(song.artist)}").decodeToString())
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            android.util.Log.w("flint", "lrclib search failed: $e")
            return Lookup.Failed
        }
        val candidates = (0 until hits.length()).map(hits::getJSONObject)
            .filter { kotlin.math.abs(it.optDouble("duration", 0.0) - song.duration.toDouble()) <= 4.0 || song.duration == 0u }
            .sortedWith(compareBy({ it.optString("syncedLyrics").isEmpty() }, { kotlin.math.abs(it.optDouble("duration", 0.0) - song.duration.toDouble()) }))
        return candidates.firstNotNullOfOrNull(::pick)?.let { Lookup.Found(it) } ?: Lookup.Missing
    }

    private fun pick(o: JSONObject): Lyrics? {
        if (o.optBoolean("instrumental")) return null
        val synced = o.optString("syncedLyrics").takeIf { it.isNotBlank() && it != "null" }
        val plain = o.optString("plainLyrics").takeIf { it.isNotBlank() && it != "null" }
        return (synced ?: plain)?.let(::lyricsFromLrc)?.takeIf { it.lines.isNotEmpty() }
    }
}
