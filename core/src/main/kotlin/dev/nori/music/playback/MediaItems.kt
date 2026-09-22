package dev.nori.music.playback

import android.net.Uri
import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import dev.nori.music.ffi.RadioStation
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
 * What the player itself carries for a song: its id, and what the system's notification and lock screen
 * show. Everything else about it (ReplayGain, the transition planner's window, the queue as the app
 * lists it, the queue saved for next time) the core keeps by id - see [toMediaItems], which hands the
 * songs to it, and crates/core/src/queue.rs.
 */
fun Song.toMediaItem(coverUrl: String?): MediaItem = MediaItem.Builder()
    .setMediaId(id)
    .setUri(songUri(id))
    .setMediaMetadata(
        MediaMetadata.Builder()
            .setTitle(title).setArtist(artist).setAlbumTitle(album)
            .setArtworkUri(coverUrl?.let(Uri::parse))
            .setDurationMs(duration.toLong() * 1000)
            .setTrackNumber(track.toInt()).setGenre(genre)
            .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC).setIsPlayable(true).setIsBrowsable(false)
            .build()
    )
    .build()

/** Songs about to be queued: handed to the core in one call, and made into the player's items. */
fun List<Song>.toMediaItems(coverUrl: (Song) -> String?): List<MediaItem> {
    if (isNotEmpty()) dev.nori.music.ffi.queueRegister(this)
    return map { it.toMediaItem(coverUrl(it)) }
}

/**
 * The song behind a queued item, from the core. An item a system controller added from outside that the
 * core never saw comes back with what the player itself knows.
 */
fun MediaItem.toSong(): Song = dev.nori.music.ffi.queueSong(mediaId) ?: Song(
    id = mediaId, title = mediaMetadata.title?.toString().orEmpty(), album = mediaMetadata.albumTitle?.toString().orEmpty(),
    artist = mediaMetadata.artist?.toString().orEmpty(), albumId = null, artistId = null, coverArt = null,
    duration = ((mediaMetadata.durationMs ?: 0L) / 1000).toUInt(), track = (mediaMetadata.trackNumber ?: 0).toUInt(),
    discNumber = 0u, year = 0u, genre = mediaMetadata.genre?.toString(), suffix = "", contentType = "", bitRate = 0u, size = 0u,
    samplingRate = 0u, bitDepth = 0u, userRating = 0u, starred = false, isExternal = false, replayGain = null,
    artists = emptyList(), created = null, playCount = 0u, played = null, path = null, explicitStatus = "",
    channelCount = 0u, musicBrainzId = null, bpm = 0u, comment = null,
)

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

