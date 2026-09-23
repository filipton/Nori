package dev.nori.music.playback

import android.net.Uri
import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import dev.nori.music.ffi.Hand
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
    return heldMediaItems(coverUrl)
}

/**
 * Songs the core handed out for the queue itself and already keeps (the saved queue, autofill, the
 * offline bridge): made into the player's items without crossing back.
 */
fun List<Song>.heldMediaItems(coverUrl: (Song) -> String?): List<MediaItem> = map { it.toMediaItem(coverUrl(it)) }

const val RADIO_PREFIX = "radio:"

fun RadioStation.toMediaItem(): MediaItem = MediaItem.Builder()
    .setMediaId(RADIO_PREFIX + id)
    .setUri(streamUrl)
    .setRequestMetadata(MediaItem.RequestMetadata.Builder().setMediaUri(Uri.parse(streamUrl)).build())
    .setMediaMetadata(
        MediaMetadata.Builder().setTitle(name).setArtist(dev.nori.music.ffi.wordsRadioArtist()).setMediaType(MediaMetadata.MEDIA_TYPE_RADIO_STATION)
            .setIsPlayable(true).setIsBrowsable(false).build()
    )
    .build()

val MediaItem.isRadio get() = mediaId.startsWith(RADIO_PREFIX)

/**
 * Songs added by hand carry how they came across the controller: [Hand.NEXT] (Play next) or [Hand.LAST]
 * (Add to queue). Where that puts them is the core's (nori_player::playlist::Playlist::take); once in,
 * both count alike as hand-added.
 */
private const val QUEUED = "queued"
fun MediaItem.queuedAs(): Hand? = mediaMetadata.extras?.getString(QUEUED)?.let { runCatching { Hand.valueOf(it) }.getOrNull() }
fun MediaItem.queued(how: Hand): MediaItem = withExtra { putString(QUEUED, how.name) }

/**
 * A list already put in the order it plays (a weighted shuffle, which the player's own shuffle would
 * undo), marked on its first item so the core keeps shuffle shown while the player's is off.
 */
private const val ORDERED = "ordered"
fun MediaItem.inOrder(): Boolean = mediaMetadata.extras?.getBoolean(ORDERED) == true
fun MediaItem.ordered(): MediaItem = withExtra { putBoolean(ORDERED, true) }

private inline fun MediaItem.withExtra(put: Bundle.() -> Unit): MediaItem =
    buildUpon().setMediaMetadata(mediaMetadata.buildUpon().setExtras(Bundle(mediaMetadata.extras ?: Bundle.EMPTY).apply(put)).build()).build()

/** A controller's items arrive without their URI; put it back. */
fun MediaItem.playable(): MediaItem =
    buildUpon().setUri(requestMetadata.mediaUri ?: songUri(mediaId)).build()

