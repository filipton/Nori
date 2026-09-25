package dev.nori.music.app.ui

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every dialog and sheet goes through Overlays.kt, so it moves in and out on the app's clock and leaves
 * before it is dropped. A raw one pops: a Compose Dialog window follows Android's window animation scale,
 * and anything composed as `if (open) AlertDialog(...)` is gone the frame `open` turns false.
 */
class OverlaysTest {
    private val ui = listOf(File("src/main/kotlin/dev/nori/music/app/ui"), File("app/src/main/kotlin/dev/nori/music/app/ui")).first { it.isDirectory }

    private val raw = Regex("""\b(AlertDialog|BasicAlertDialog|ModalBottomSheet|DatePickerDialog|TimePickerDialog)\s*\(|window\.Dialog\s*\(|\bDialog\s*\(\s*(\{|onDismissRequest)""")

    @Test fun `no screen puts up a dialog or sheet of its own`() {
        val found = ui.walkTopDown().filter { it.isFile && it.extension == "kt" && it.name != "Overlays.kt" }.flatMap { f ->
            f.readLines().mapIndexedNotNull { i, line ->
                val code = line.substringBefore("//")
                if (raw.containsMatchIn(code)) "${f.name}:${i + 1}: ${line.trim()}" else null
            }
        }.toList()
        assertEquals("use NoriDialog / NoriSheet (ui/Overlays.kt) instead:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    @Test fun `the dialog windows have no animation or dim of their own`() {
        val res = listOf(File("src/main/res/values/themes.xml"), File("app/src/main/res/values/themes.xml")).first { it.isFile }.readText()
        assertTrue(res.contains("""<item name="android:windowAnimationStyle">@null</item>"""))
        assertTrue(res.contains("""<item name="android:backgroundDimEnabled">false</item>"""))
        val manifest = listOf(File("src/main/AndroidManifest.xml"), File("app/src/main/AndroidManifest.xml")).first { it.isFile }.readText()
        assertTrue(manifest.contains("""android:theme="@style/Theme.Nori""""))
    }
}
