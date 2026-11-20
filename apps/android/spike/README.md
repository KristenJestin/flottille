# The push spike (#823)

A throwaway proof, never shipped: that a phone answers a request of an agent from its
notification, with the app killed and the phone in Doze (`docs/remote.md`, "The Android app",
first thing proven, and "Notifications"). The real app (#824, #828) is built apart; this folder
goes once they stand.

- `app/`: the phone's side, Kotlin only (no NDK). It makes its Ed25519 device key (BouncyCastle,
  sealed under an Android Keystore key), joins through the hub as a controller (the hub's control
  session of `crates/flottille-link/src/control.rs`, spoken in Kotlin), registers with the
  UnifiedPush distributor installed (the connector's side of the spec, written here) and gives the
  runner its endpoint and keys in a signed `push.register`. On a push it decrypts the Web Push
  (RFC 8291), draws the notification with Allow and Deny at once from the kind and the id, and
  starts an expedited job that tries to read the request. Allow and Deny are a broadcast receiver
  with `goAsync`: it signs `approval.approve` or `approval.deny` and sends it to the gate.
- `distributor/`: a stub distributor for the emulator, which hands the pushes the server holds to
  the app as ntfy does on a phone. Its instrumentation tests are the proof (`PushInDozeTest`).
- `server/`: the test machine's side, a crate outside the workspace: a hub, a daemon of
  temporary folders whose agent is the fake one, the **gate** the phone answers to, and the
  sender of the push. The gate stands for the link: what the phone sends it is a request signed
  in the very encoding a link carries, checked by `flottille_link::Requests::check` against the
  member list the daemon checked, then asked of the daemon.
- `ci.sh`: the whole proof on a GitHub runner's emulator. `serve.sh`, `measure.py` and
  [`MEASURE.md`](MEASURE.md): the one run on a real phone.

What it does not prove: the QUIC link from the phone (the gate is plain HTTP, the request signed
but not encrypted), which the mobile core brings with the NDK (#824); the member list's
signatures, which the app does not check here.
