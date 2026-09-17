package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.flint.music.Flint
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
 * ViewModels hold every decision; composables only draw their state and call
 * their functions. Replacing the UI means replacing the ui package, nothing else.
 */
abstract class FlintViewModel(app: Application) : AndroidViewModel(app) {
    protected val flint: Flint = Flint.get(app)

    /** Collected only while a screen is looking, and for five seconds after, so a rotation does not refetch. */
    protected fun <T> Flow<T>.asLoad(): StateFlow<Load<T>> =
        map<T, Load<T>> { Load.Ready(it) }
            .catch { emit(Load.Failed(it.message ?: it.javaClass.simpleName)) }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), Load.Loading)

    fun cover(id: String?, size: Int): String? = flint.library.coverUrl(id, size)
}
