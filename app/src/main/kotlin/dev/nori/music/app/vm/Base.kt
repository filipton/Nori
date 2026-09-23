package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.nori.music.Nori
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn

/** What a screen is showing while its data is on the way. */
sealed interface Load<out T> {
    data object Loading : Load<Nothing>
    data class Ready<T>(val data: T) : Load<T>
    data class Failed(val message: String) : Load<Nothing>
}

/**
 * A list that pages grow at the end. Each page used to copy everything before it into a new list (the
 * thousandth page of a long list copied 999 pages to add one); here each page is added to one shared
 * array and published as a new view of it, whose [size] fixes what it shows, so a page costs its own
 * length. Views already handed out never change: only the newest view appends in place, and anything
 * else (a page answered twice, a stale view growing) starts a new array.
 *
 * Equal only to itself: a new page is always a change, and comparing two long lists element by element
 * on every page is the cost this exists to avoid.
 */
class Grown<T> private constructor(private val backing: ArrayList<T>, override val size: Int) : AbstractList<T>(), RandomAccess {
    override fun get(index: Int): T {
        if (index < 0 || index >= size) throw IndexOutOfBoundsException("$index of $size")
        return backing[index]
    }

    /** This list with [page] in place of everything from [offset] on. */
    fun from(offset: Int, page: List<T>): Grown<T> {
        val keep = minOf(offset, size)
        if (keep == size && size == backing.size) {
            backing.addAll(page)
            return Grown(backing, backing.size)
        }
        val fresh = ArrayList<T>(keep + page.size)
        fresh.addAll(backing.subList(0, keep))
        fresh.addAll(page)
        return Grown(fresh, fresh.size)
    }

    /** This list with [page] after it. */
    operator fun plus(page: List<T>): Grown<T> = from(size, page)

    override fun equals(other: Any?): Boolean = this === other
    override fun hashCode(): Int = System.identityHashCode(this)

    companion object {
        fun <T> empty(): Grown<T> = Grown(ArrayList(), 0)
    }
}

/**
 * ViewModels hold every decision; composables only draw their state and call
 * their functions. Replacing the UI means replacing the ui package, nothing else.
 */
abstract class NoriViewModel(app: Application) : AndroidViewModel(app) {
    protected val nori: Nori = Nori.get(app)

    /** Collected only while a screen is looking, and for five seconds after, so a rotation does not refetch. */
    protected fun <T> Flow<T>.asLoad(): StateFlow<Load<T>> =
        map<T, Load<T>> { Load.Ready(it) }
            .catch { emit(Load.Failed(it.message ?: it.javaClass.simpleName)) }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), Load.Loading)

    fun cover(id: String?, size: Int): String? = nori.library.coverUrl(id, size)
}
