package dev.nori.music.app.perf

import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.net.Uri
import dev.nori.music.app.PerfHooks

/**
 * Starts the recorder with the process, before the application and its first activity, so the first
 * stretch begins at the start. A provider rather than a line in the application: the perf build's
 * manifest declares it, and no other build has it. It serves nothing.
 */
class PerfStart : ContentProvider() {
    override fun onCreate(): Boolean {
        val app = context?.applicationContext as? Application ?: return false
        PerfHooks.recorder = Recorder(app).also { it.install() }
        return true
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?) = null
    override fun getType(uri: Uri): String? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
}
