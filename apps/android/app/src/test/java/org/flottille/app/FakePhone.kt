package org.flottille.app

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import org.flottille.core.HubCard
import org.flottille.core.Phone
import org.flottille.core.PhoneException
import org.flottille.core.PhoneView

/**
 * The core the screens see, played: what the tests give it is what the screens draw, and what the
 * screens ask of it is kept. The real core stands behind the same interface in the app.
 */
class FakePhone(initial: PhoneView = PhoneView()) : Phone {
    val state = MutableStateFlow(initial)

    /** The card a code gives; a code that is not here is refused as the core refuses it. */
    val cards = mutableMapOf("CODE-1" to HubCard("https://hub.example.net", "SHA256:q1w2e3r4"))

    /** What the view is once the phone joined. */
    var afterJoin = PhoneView(role = "controller", name = "pixel")

    /** What the view is once the phone left. */
    var afterLeave = PhoneView(said = listOf("this phone was removed from the user's hub: pair it again"))

    data class Joined(val code: String, val name: String, val role: String)

    val joins = mutableListOf<Joined>()
    var leaves = 0

    override val view: PhoneView get() = state.value

    override fun views(): Flow<PhoneView> = state

    override fun card(code: String): HubCard =
        cards[code.trim()] ?: throw PhoneException("this is not a Flottille pair code")

    override suspend fun join(code: String, name: String, role: String) {
        joins += Joined(code, name, role)
        state.value = afterJoin.copy(role = role.ifEmpty { afterJoin.role })
    }

    override suspend fun leave() {
        leaves += 1
        state.value = afterLeave
    }

}
