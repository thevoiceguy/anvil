package com.thevoiceguy.anvil

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.MethodChannel

/**
 * Whether the phone's network is cellular (not Wi-Fi or a wire), told to
 * Dart at every change as `network` `{cellular}` (lib/src/mobile): on
 * cellular a call sends at most 360p, or no video with the user's Wi-Fi
 * only setting.
 */
class NetworkWatch(context: Context, private val channel: MethodChannel) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var told: Boolean? = null

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            val now = cellular(caps)
            main.post {
                if (told != now) {
                    told = now
                    channel.invokeMethod("network", mapOf("cellular" to now))
                }
            }
        }
    }

    init {
        connectivity.registerDefaultNetworkCallback(callback)
    }

    /** Now, for Dart's first ask; null with no network. */
    fun cellular(): Boolean? =
        connectivity.activeNetwork?.let { connectivity.getNetworkCapabilities(it) }?.let { cellular(it) }

    fun stop() = connectivity.unregisterNetworkCallback(callback)

    private fun cellular(caps: NetworkCapabilities) =
        caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) &&
            !caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) &&
            !caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
}
