package org.flottille.app

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import org.flottille.core.Machine
import org.flottille.core.MachineState

private val clock = DateTimeFormatter.ofPattern("HH:mm")

/** The time of day of a moment, in the user's zone: `22:14`. */
fun timeOfDay(millis: Long, zone: ZoneId): String = clock.withZone(zone).format(Instant.ofEpochMilli(millis))

/** A machine's state, said once: `Online`, `Offline since 22:14`, `Needs an update`. */
fun stateLine(machine: Machine, zone: ZoneId): String =
    when (machine.state) {
        MachineState.Online -> "Online"
        MachineState.Offline -> "Offline since ${timeOfDay(machine.since, zone)}"
        MachineState.NeedsUpdate -> "Needs an update"
    }

/** How long ago a moment was, as a row at rest says it: `now`, `5 min`, `3 h`, `2 d`. */
fun ago(millis: Long, now: Long): String {
    val minutes = (now - millis).coerceAtLeast(0) / 60_000
    return when {
        minutes < 1 -> "now"
        minutes < 60 -> "$minutes min"
        minutes < 24 * 60 -> "${minutes / 60} h"
        else -> "${minutes / (24 * 60)} d"
    }
}
