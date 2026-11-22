package com.sonimesh.opusspike

import android.net.InetAddresses
import java.net.Inet4Address

internal const val DEFAULT_LAB_ADDRESS = "10.0.2.2"

internal fun parseLabAddress(text: String): Inet4Address? {
    val address = try { InetAddresses.parseNumericAddress(text.trim()) } catch (_: IllegalArgumentException) { return null }
    if (address !is Inet4Address || address.isAnyLocalAddress || address.isLoopbackAddress ||
        address.isMulticastAddress || address.hostAddress == "255.255.255.255") return null
    return address
}
