package com.sonimesh.opusspike

import android.net.InetAddresses
import java.net.Inet4Address

internal const val DEFAULT_LAB_ADDRESS = "10.0.2.2"
internal const val DEFAULT_LAB_MEDIA_PORT = 39093

internal fun parseLabMediaPort(text: String): Int? {
    if (text.isEmpty() || text.length > 5 || text.any { it !in '0'..'9' }) return null
    return text.toIntOrNull()?.takeIf { it in 1..65535 }
}

internal fun parseLabAddress(text: String): Inet4Address? {
    val address = try { InetAddresses.parseNumericAddress(text.trim()) } catch (_: IllegalArgumentException) { return null }
    if (address !is Inet4Address || address.isAnyLocalAddress || address.isLoopbackAddress ||
        address.isMulticastAddress || address.hostAddress == "255.255.255.255") return null
    return address
}
