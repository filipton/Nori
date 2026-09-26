package dev.nori.music.app.vm

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * What a long press on a row picks, to act on together (the selection bar). It belongs to the page it
 * was made on: back lets go of it before it leaves the page, and any change of page ends it, so the bar
 * never stays up over a page where nothing is picked. Plain state, tested on the JVM (SelectionTest).
 */
class Selection<T>(private val id: (T) -> String) {
    private val _items = MutableStateFlow<List<T>>(emptyList())
    val items: StateFlow<List<T>> = _items
    private var page: String? = null

    /** Picks [item], or puts it back if it was picked. */
    fun toggle(item: T) = _items.update { s -> if (s.any { id(it) == id(item) }) s.filterNot { id(it) == id(item) } else s + item }

    fun clear() { _items.value = emptyList() }

    /** Back was pressed: with something picked it is let go of and the press goes no further (true). */
    fun back(): Boolean = _items.value.isNotEmpty().also { if (it) clear() }

    /** The page on screen is now [key]; a different one than before ends the selection. */
    fun onPage(key: String) {
        if (page != null && page != key) clear()
        page = key
    }
}
