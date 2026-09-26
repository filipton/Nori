package dev.nori.music.app.ui

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Every song row is a lazy item of its own, so a list composes only the rows on screen. A page that drew its
 * songs in a loop inside one item (an album's, a playlist's, as the pages under a hero did) composed and
 * measured all of them at once, in the frames of the page's slide in: a playlist of a thousand songs
 * stuttered as it opened. The app has no Compose UI tests on the JVM, so this reads the sources: going back
 * from each `SongRow(`, the first list construct met must be a lazy `items`/`itemsIndexed`, not a `forEach`
 * or a single `item`.
 */
class SongListsTest {
    private val ui = listOf(File("src/main/kotlin/dev/nori/music/app/ui"), File("app/src/main/kotlin/dev/nori/music/app/ui")).first { it.isDirectory }

    private val lazy = Regex("""\b(items|itemsIndexed)\s*\(""")
    private val eager = Regex("""\.forEach(Indexed)?\s*\{|\bitem\s*\(|\brepeat\s*\(|\bfor\s*\(""")

    /** A call of `SongRow(`: where it is, and the list construct nearest above it in its function (null: none). */
    private class Site(val at: String, val around: String?, val isLazy: Boolean)

    private fun sites(): List<Site> = ui.walkTopDown().filter { it.isFile && it.extension == "kt" }.flatMap { f ->
        val lines = f.readLines().map { it.substringBefore("//") }
        lines.indices.filter { Regex("""\bSongRow\s*\(""").containsMatchIn(lines[it]) && !lines[it].contains("fun SongRow") }.map { at ->
            val above = (at downTo 0).asSequence().takeWhile { it == at || !Regex("""\bfun\s""").containsMatchIn(lines[it]) }.map { lines[it] }
            val found = above.firstNotNullOfOrNull { line -> lazy.find(line)?.let { it.value to true } ?: eager.find(line)?.let { it.value to false } }
            Site("${f.name}:${at + 1}", found?.first, found?.second == true)
        }
    }.toList()

    @Test fun `every song row is a lazy item of its own`() {
        val all = sites()
        assertTrue("no SongRow call found; has the row been renamed?", all.isNotEmpty())
        val wrong = all.filterNot { it.isLazy }.map { "${it.at}: inside ${it.around ?: "no lazy list"}" }
        assertEquals(
            "a song row inside a loop or a single item composes every row of the list at once; use songRows (Components.kt):\n" + wrong.joinToString("\n"),
            emptyList<String>(), wrong,
        )
    }
}
