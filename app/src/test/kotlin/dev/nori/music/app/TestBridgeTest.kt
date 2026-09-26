package dev.nori.music.app

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The test bridge is the debug build's alone: src/main reaches it only through the names both twins of
 * TestDriver.kt declare (src/debug has the real one, src/noTest the empty one release and perf build).
 */
class TestBridgeTest {
    private fun dir(path: String) = listOf(File(path), File("app/$path")).first { it.exists() }
    private val main = dir("src/main/kotlin")

    @Test fun `nothing in main reaches the bridge but through TestDriver`() {
        val bridge = Regex("""\b(TestHooks|TestBridge|TestActions)\b""")
        val found = main.walkTopDown().filter { it.isFile && it.extension == "kt" }.flatMap { f ->
            f.readLines().mapIndexedNotNull { i, line -> if (bridge.containsMatchIn(line.substringBefore("//"))) "${f.name}:${i + 1}: ${line.trim()}" else null }
        }.toList()
        assertEquals("the test bridge belongs in src/debug:\n" + found.joinToString("\n"), emptyList<String>(), found)
    }

    @Test fun `both twins of TestDriver declare the same names`() {
        val decl = Regex("""^(?:inline |const )?(?:fun|val) (?:<[^>]+> )?(\w+)""", RegexOption.MULTILINE)
        fun names(f: File) = decl.findAll(f.readText()).map { it.groupValues[1] }.toSortedSet()
        val debug = names(dir("src/debug/kotlin/dev/nori/music/app/TestDriver.kt"))
        val none = names(dir("src/noTest/kotlin/dev/nori/music/app/TestDriver.kt"))
        assertEquals(debug, none)
    }
}
