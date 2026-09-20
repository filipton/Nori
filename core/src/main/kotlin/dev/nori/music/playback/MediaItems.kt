package dev.nori.music.playback

import android.net.Uri
import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import dev.nori.music.ffi.RadioStation
import dev.nori.music.ffi.ReplayGain
import dev.nori.music.ffi.Song

/**
 * The artwork size asked for on the lock screen and in the notification. The same number the
 * full-screen player uses, so the server renders and caches one large rendition per cover rather than
 * one for each place it is shown - each new size costs a slow first fetch on a real library.
 */
const val NOTIFICATION_ART = 800


/** Audio is addressed as nori://song/<id>; the real URL is decided when the bytes are needed. */
const val SONG_SCHEME = "nori"

fun songUri(id: String): Uri = Uri.Builder().scheme(SONG_SCHEME).authority("song").appendPath(id).build()

/**
 * The song rides along in the metadata extras, so the service can rebuild it
 * (ReplayGain, scrobbling, queue persistence) without asking the server again.
 */
fun Song.toMediaItem(coverUrl: String?): MediaItem {
    val extras = Bundle().apply {
        putString("album", album); putString("albumId", albumId); putString("artistId", artistId); putString("coverArt", coverArt)
        putInt("duration", duration.toInt()); putInt("track", track.toInt()); putInt("disc", discNumber.toInt()); putInt("year", year.toInt())
        putString("suffix", suffix); putString("contentType", contentType); putInt("bitRate", bitRate.toInt()); putLong("size", size.toLong())
        putInt("samplingRate", samplingRate.toInt()); putInt("bitDepth", bitDepth.toInt())
        putBoolean("starred", starred); putInt("rating", userRating.toInt()); putBoolean("external", isExternal); putString("explicit", explicitStatus)
        replayGain?.let { g ->
            g.trackGain?.let { putFloat("trackGain", it) }; g.albumGain?.let { putFloat("albumGain", it) }
            g.trackPeak?.let { putFloat("trackPeak", it) }; g.albumPeak?.let { putFloat("albumPeak", it) }
        }
    }
    return MediaItem.Builder()
        .setMediaId(id)
        .setUri(songUri(id))
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(title).setArtist(artist).setAlbumTitle(album)
                .setArtworkUri(coverUrl?.let(Uri::parse))
                .setDurationMs(duration.toLong() * 1000)
                .setTrackNumber(track.toInt()).setGenre(genre)
                .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC).setIsPlayable(true).setIsBrowsable(false)
                .setExtras(extras)
                .build()
        )
        .build()
}

fun MediaItem.toSong(): Song {
    val m = mediaMetadata
    val e = m.extras ?: Bundle.EMPTY
    fun f(k: String) = if (e.containsKey(k)) e.getFloat(k) else null
    val gain = ReplayGain(f("trackGain"), f("albumGain"), f("trackPeak"), f("albumPeak"))
    return Song(
        id = mediaId, title = m.title?.toString().orEmpty(), album = e.getString("album") ?: m.albumTitle?.toString().orEmpty(),
        artist = m.artist?.toString().orEmpty(), albumId = e.getString("albumId"), artistId = e.getString("artistId"),
        coverArt = e.getString("coverArt"), duration = e.getInt("duration").toUInt(), track = e.getInt("track").toUInt(),
        discNumber = e.getInt("disc").toUInt(), year = e.getInt("year").toUInt(), genre = m.genre?.toString(),
        suffix = e.getString("suffix").orEmpty(), contentType = e.getString("contentType").orEmpty(),
        bitRate = e.getInt("bitRate").toUInt(), size = e.getLong("size").toULong(),
        samplingRate = e.getInt("samplingRate").toUInt(), bitDepth = e.getInt("bitDepth").toUInt(),
        userRating = e.getInt("rating").toUByte(), starred = e.getBoolean("starred"), isExternal = e.getBoolean("external"),
        replayGain = gain.takeIf { it.trackGain != null || it.albumGain != null },
        artists = emptyList(), created = null, playCount = 0u, played = null, path = null, explicitStatus = e.getString("explicit").orEmpty(),
        channelCount = 0u, musicBrainzId = null, bpm = 0u, comment = null,
    )
}

const val RADIO_PREFIX = "radio:"

fun RadioStation.toMediaItem(): MediaItem = MediaItem.Builder()
    .setMediaId(RADIO_PREFIX + id)
    .setUri(streamUrl)
    .setRequestMetadata(MediaItem.RequestMetadata.Builder().setMediaUri(Uri.parse(streamUrl)).build())
    .setMediaMetadata(
        MediaMetadata.Builder().setTitle(name).setArtist("Radio").setMediaType(MediaMetadata.MEDIA_TYPE_RADIO_STATION)
            .setIsPlayable(true).setIsBrowsable(false).build()
    )
    .build()

val MediaItem.isRadio get() = mediaId.startsWith(RADIO_PREFIX)

/** A controller's items arrive without their URI; put it back. */
/**
 * Songs added by hand carry how they came: "next" (Play next) or "last" (Add to queue). The service
 * keeps them right after the playing song, in the order they were added and ahead of the rest of the
 * queue, shuffled or not (see PlaybackService.upNext). Once in, both count alike as hand-added.
 */
private const val QUEUED = "queued"
fun MediaItem.queuedAs(): String? = mediaMetadata.extras?.getString(QUEUED)
fun MediaItem.queued(how: String): MediaItem =
    buildUpon().setMediaMetadata(mediaMetadata.buildUpon().setExtras(Bundle(mediaMetadata.extras ?: Bundle.EMPTY).apply { putString(QUEUED, how) }).build()).build()

fun MediaItem.playable(): MediaItem =
    buildUpon().setUri(requestMetadata.mediaUri ?: songUri(mediaId)).build()
