package org.flottille.app

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import org.junit.Rule
import org.junit.Test

/**
 * The app on the emulator, with the real core (the library built for it, the Android Keystore):
 * a phone that joined no hub starts on Connect to your hub. The camera is none there; the code is
 * pasted in the screens' own tests.
 */
class AppOnThePhoneTest {
    @get:Rule
    val rule = createAndroidComposeRule<MainActivity>()

    @Test
    fun a_phone_that_joined_no_hub_starts_on_connect_to_your_hub() {
        rule.onNodeWithText("Connect to your hub").assertIsDisplayed()
    }
}
