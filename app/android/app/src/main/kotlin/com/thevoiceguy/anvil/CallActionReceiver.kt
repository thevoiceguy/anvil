package com.thevoiceguy.anvil

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Decline or Hang up pressed on the call's notification. */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != CallService.END) return
        val call = intent.getLongExtra(CallService.EXTRA_CALL, -1)
        if (call >= 0) CallSystem.emit("end", call)
    }
}
