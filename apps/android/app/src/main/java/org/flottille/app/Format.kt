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

/** A session's state, as its row says it. */
fun sessionState(state: String, waiting: Boolean): String =
    when {
        waiting -> "Waiting for you"
        state == "working" -> "Working"
        state == "starting" -> "Starting"
        state == "ended" -> "Ended"
        else -> "Idle"
    }
