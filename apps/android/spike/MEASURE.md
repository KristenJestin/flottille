# The real-phone run of the push spike (#823)

Once, on a real phone: ten tries on Wi-Fi, ten on 4G. About 30 minutes. The numbers go in the
pull request's table. Nothing here installs anything on the computer: the build tools are the
ones Android Studio has, and Gradle is downloaded into `~/.cache/flottille-spike`. Everything
the server makes is in a temporary folder of its own, never your configuration, keyring or home.

## What you need

- the phone, USB debugging on, plugged in (`adb devices` lists it);
- **ntfy** on the phone, from F-Droid or the Play Store: it is the UnifiedPush distributor. Open it
  once and allow it to ignore battery optimisations when it asks (a distributor must stay
  reachable in Doze; the spike's app is not exempted, which is what is tested);
- for 4G: an address of the computer the phone reaches from a mobile network (a forwarded port to
  7470, or a VPN the phone and the computer are on). On Wi-Fi, the computer's LAN address.

## Steps

From the repository's root, on the branch of the pull request.

1. Build and install the app:

   ```
   apps/android/spike/gradle.sh :app:assembleDebug
   adb install -r apps/android/spike/app/build/outputs/apk/debug/app-debug.apk
   ```

2. In a terminal of its own, start the server side (a hub, a daemon with the fake agent, the
   gate on port 7470) and leave it running:

   ```
   apps/android/spike/serve.sh
   ```

3. Wi-Fi: phone on the same Wi-Fi as the computer, mobile data off.

   ```
   apps/android/spike/measure.py --network wifi --gate http://<computer's LAN address>:7470
   ```

   The first time, it joins: the app opens, shows four words and joins as a controller (the
   server accepts on your behalf), then registers with ntfy. Then each of the ten tries kills the
   app, forces Doze (`adb shell dumpsys deviceidle force-idle`) and turns the screen off; when the
   terminal says so, **wake the screen and tap Allow** on the notification (without unlocking).

4. 4G: Wi-Fi off on the phone, mobile data on.

   ```
   apps/android/spike/measure.py --network 4g --gate http://<address reachable from 4G>:7470
   ```

5. Paste `measure-wifi.md` and `measure-4g.md` into the pull request's table, and fill the
   choice for #828 there. Stop the server with Ctrl+C; `adb uninstall org.flottille.spike` and the
   folder `/tmp/flottille-spike-*` it named can go.

## What the columns say

- **push → notification**: from the server sending the Web Push to the app drawing its
  notification, the phone's clock set against the computer's over adb (a few ms off at most);
- **tap → answer**: from the tap on Allow to the gate's answer, on the phone's clock: the request
  signed, sent, checked against the member list and answered by the daemon;
- **read in Doze**: whether the expedited job the push starts could reach the gate while the phone
  was in Doze, to read the request's details (`refused` means the network was cut);
- **Doze**: `IDLE` when the phone was in deep Doze at the push.

If something fails, the app's screen and `adb logcat -s FlottilleSpike` say what; the server's
terminal prints every event with its time.
