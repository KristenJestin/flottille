package org.flottille.spike

import android.content.Context
import android.content.SharedPreferences
import android.util.Log

/** What the spike keeps between its processes, and its log (`adb logcat -s FlottilleSpike`). */
object Store {
    const val TAG = "FlottilleSpike"

    fun prefs(context: Context): SharedPreferences =
        context.getSharedPreferences("spike", Context.MODE_PRIVATE)

    fun log(context: Context, what: String) {
        Log.i(TAG, what)
        val prefs = prefs(context)
        val lines = (prefs.getString("log", "") ?: "").lines().takeLast(30)
        prefs.edit().putString("log", (lines + what).joinToString("\n").trim()).commit()
    }

    fun status(context: Context, what: String) {
        prefs(context).edit().putString("status", what).commit()
        log(context, what)
    }
}
