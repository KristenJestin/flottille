package org.flottille.spike.distributor

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import java.net.HttpURLConnection
import java.net.URL

/**
 * The stub distributor: a registration is answered at once with an endpoint on the spike's
 * server (`<server>/push/<token>`), and `Relay` hands each push the server holds to the app, as
 * a distributor does, with the bytes as they came (still encrypted).
 */
object Distributor {
    const val TAG = "SpikeDistributor"

    fun prefs(context: Context) = context.getSharedPreferences("distributor", Context.MODE_PRIVATE)

    fun server(context: Context): String = prefs(context).getString("server", "http://10.0.2.2:7470")!!
}

class RegisterReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val token = intent.getStringExtra("token") ?: return
        when (intent.action) {
            "org.unifiedpush.android.distributor.REGISTER" -> {
                // The app that registers: shared on Android 14 and later, else the PendingIntent's.
                val shared = if (android.os.Build.VERSION.SDK_INT >= 34) sentFromPackage else null
                val app = shared ?: intent.getStringExtra("application")
                    ?: (intent.getParcelableExtra("pi", android.app.PendingIntent::class.java))?.creatorPackage
                    ?: return
                Distributor.prefs(context).edit().putString("app.$token", app).putString("token", token).commit()
                val endpoint = "${Distributor.server(context)}/push/$token"
                Log.i(Distributor.TAG, "registered $app at $endpoint")
                context.sendBroadcast(
                    Intent("org.unifiedpush.android.connector.NEW_ENDPOINT").setPackage(app)
                        .putExtra("token", token).putExtra("endpoint", endpoint).putExtra("id", token),
                )
            }
            "org.unifiedpush.android.distributor.MESSAGE_ACK" ->
                Log.i(Distributor.TAG, "acknowledged ${intent.getStringExtra("id")}")
            else -> Log.i(Distributor.TAG, "${intent.action} for $token")
        }
    }
}

/** Holds a long poll on the server while it runs, and hands each push to the app it is for. */
class Relay(private val context: Context) : Thread("relay") {
    @Volatile var stopped = false
    @Volatile var relayed = 0

    override fun run() {
        while (!stopped) {
            val prefs = Distributor.prefs(context)
            val token = prefs.getString("token", null)
            if (token == null) {
                sleep(200)
                continue
            }
            try {
                val connection = URL("${Distributor.server(context)}/poll?token=$token").openConnection() as HttpURLConnection
                connection.connectTimeout = 5_000
                connection.readTimeout = 30_000
                val code = connection.responseCode
                if (code == 200) {
                    val bytes = connection.inputStream.readBytes()
                    val app = prefs.getString("app.$token", null) ?: continue
                    context.sendBroadcast(
                        Intent("org.unifiedpush.android.connector.MESSAGE").setPackage(app)
                            .putExtra("token", token).putExtra("bytesMessage", bytes)
                            .putExtra("id", "m${System.nanoTime()}"),
                    )
                    relayed++
                    Log.i(Distributor.TAG, "relayed ${bytes.size} bytes to $app")
                }
                connection.disconnect()
            } catch (error: Exception) {
                Log.i(Distributor.TAG, "poll: $error")
                sleep(500)
            }
        }
    }
}
