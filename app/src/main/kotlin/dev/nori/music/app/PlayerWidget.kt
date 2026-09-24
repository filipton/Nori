package dev.nori.music.app

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.view.KeyEvent
import android.widget.RemoteViews
import androidx.media3.session.MediaButtonReceiver
import dev.nori.music.playback.PlaybackService

/**
 * Home-screen player. It never polls (updatePeriodMillis is 0): the playback service announces
 * track and play-state changes, and that broadcast is the only thing that redraws it. Buttons are
 * plain media-button intents, so they work with the app and the service both dead.
 */
class PlayerWidget : AppWidgetProvider() {
    override fun onReceive(context: Context, intent: Intent) {
        super.onReceive(context, intent)
        if (intent.action == PlaybackService.ACTION_STATE) {
            last = State(intent.getStringExtra(PlaybackService.EXTRA_TITLE), intent.getStringExtra(PlaybackService.EXTRA_ARTIST), intent.getBooleanExtra(PlaybackService.EXTRA_PLAYING, false))
            val manager = AppWidgetManager.getInstance(context)
            onUpdate(context, manager, manager.getAppWidgetIds(ComponentName(context, PlayerWidget::class.java)))
        }
    }

    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        if (ids.isEmpty()) return
        val views = RemoteViews(context.packageName, R.layout.widget_player).apply {
            setTextViewText(R.id.widget_title, last.title ?: dev.nori.music.ffi.words.wordsWidgetIdle())
            setTextViewText(R.id.widget_artist, last.artist.orEmpty())
            setImageViewResource(R.id.widget_toggle, if (last.playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play)
            setOnClickPendingIntent(R.id.widget_previous, key(context, KeyEvent.KEYCODE_MEDIA_PREVIOUS))
            setOnClickPendingIntent(R.id.widget_toggle, key(context, KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE))
            setOnClickPendingIntent(R.id.widget_next, key(context, KeyEvent.KEYCODE_MEDIA_NEXT))
            context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
                setOnClickPendingIntent(R.id.widget_text, PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_IMMUTABLE))
            }
        }
        manager.updateAppWidget(ids, views)
    }

    private fun key(context: Context, code: Int): PendingIntent = PendingIntent.getBroadcast(
        context, code,
        Intent(Intent.ACTION_MEDIA_BUTTON).setComponent(ComponentName(context, MediaButtonReceiver::class.java)).putExtra(Intent.EXTRA_KEY_EVENT, KeyEvent(KeyEvent.ACTION_DOWN, code)),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private data class State(val title: String? = null, val artist: String? = null, val playing: Boolean = false)

    private companion object { var last = State() }
}
