package org.flottille.app

import org.flottille.app.ui.Motion
import org.junit.Assert.assertEquals
import org.junit.Test

/** The kit's durations (`gpui-design-system`, `src/motion.rs`), copied as tokens. */
class TokensTest {
    @Test
    fun the_motion_tokens_are_the_kit_s_durations() {
        assertEquals(240, Motion.ENTER)
        assertEquals(280, Motion.PANEL)
        assertEquals(420, Motion.COVER)
        assertEquals(140, Motion.HOVER)
        assertEquals(90, Motion.PRESS)
        assertEquals(180, Motion.MODAL)
        assertEquals(70, Motion.STAGGER)
        assertEquals(8, Motion.MAX_STAGGER)
    }
}
