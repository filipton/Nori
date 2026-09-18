package dev.flint.music.net

import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HostTest {
    private fun same(url: String, address: String): Boolean {
        val m = Class.forName("dev.flint.music.net.HttpKt").getDeclaredMethod("sameHost", okhttp3.HttpUrl::class.java, String::class.java)
        m.isAccessible = true
        return m.invoke(null, url.toHttpUrl(), address) as Boolean
    }

    @Test
    fun serverHeadersOnlyReachTheServer() {
        assertTrue(same("https://music.example.com/rest/ping", "music.example.com"))
        assertTrue(same("http://10.0.2.2:4533/rest/stream?id=1", "http://10.0.2.2:4533/"))
        assertFalse("another port is another service", same("http://10.0.2.2:8080/", "http://10.0.2.2:4533"))
        assertFalse(same("https://lrclib.net/api/get", "music.example.com"))
        assertFalse(same("https://raw.githubusercontent.com/x", "music.example.com"))
        assertFalse(same("https://music.example.com.evil.net/", "music.example.com"))
        assertFalse(same("https://music.example.com/", ""))
    }
}
