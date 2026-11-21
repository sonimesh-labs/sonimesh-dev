package com.sonimesh.discoveryspike

import android.Manifest
import android.content.Context
import android.net.nsd.DiscoveryRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Opt-in Android 17 discovery probe for ADR 0014.
 *
 * The production sender stays capture-only. This test target is deliberately
 * separate and only records platform callback behavior; a discovered service
 * is never treated as trusted or used to establish a session.
 */
@RunWith(AndroidJUnit4::class)
class Android17DiscoveryProbeTest {
    @Test
    fun discovery_request_uses_the_picker_flag() {
        assumeTrue("Android 17 is required", Build.VERSION.SDK_INT >= 37)

        val request = DiscoveryRequest.Builder(SERVICE_TYPE)
            .setFlags(DiscoveryRequest.FLAG_SHOW_PICKER)
            .build()

        assertEquals(SERVICE_TYPE, request.serviceType)
        assertTrue(request.flags and DiscoveryRequest.FLAG_SHOW_PICKER != 0L)
    }

    @Test
    fun picker_probe_is_opt_in_and_reports_a_bounded_callback_outcome() {
        assumeTrue("Android 17 is required", Build.VERSION.SDK_INT >= 37)
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue(
            "picker probe requires runDiscoveryPicker=true",
            arguments.getString("runDiscoveryPicker") == "true",
        )

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val manager = requireNotNull(context.getSystemService(Context.NSD_SERVICE) as? NsdManager)
        val executor = Executors.newSingleThreadExecutor()
        val outcome = CountDownLatch(1)
        var callbackOutcome: CallbackOutcome? = null
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                callbackOutcome = CallbackOutcome.Started
                runCatching { manager.stopServiceDiscovery(this) }
            }

            override fun onDiscoveryStopped(serviceType: String) {
                callbackOutcome = CallbackOutcome.Stopped
                outcome.countDown()
            }

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                callbackOutcome = CallbackOutcome.Failed(errorCode)
                outcome.countDown()
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                callbackOutcome = CallbackOutcome.Failed(errorCode)
                outcome.countDown()
            }

            override fun onServiceFound(serviceInfo: NsdServiceInfo) = Unit

            override fun onServiceLost(serviceInfo: NsdServiceInfo) = Unit
        }

        try {
            manager.discoverServices(
                DiscoveryRequest.Builder(SERVICE_TYPE)
                    .setFlags(DiscoveryRequest.FLAG_SHOW_PICKER)
                    .build(),
                executor,
                listener,
            )
            assertTrue("discovery callback did not arrive", outcome.await(TIMEOUT_SECONDS, TimeUnit.SECONDS))
            val finalOutcome = callbackOutcome
            assertTrue("callback outcome was missing", finalOutcome != null)
            assertTrue(
                "picker probe did not reach a terminal callback: $finalOutcome",
                finalOutcome is CallbackOutcome.Stopped || finalOutcome is CallbackOutcome.Failed,
            )
            Log.i(TAG, "picker probe outcome=$finalOutcome")
        } finally {
            runCatching { manager.stopServiceDiscovery(listener) }
            outcome.await(TIMEOUT_SECONDS, TimeUnit.SECONDS)
            executor.shutdownNow()
        }
    }

    @Test
    fun broad_permission_state_is_reported_without_claiming_a_grant() {
        assumeTrue("Android 17 is required", Build.VERSION.SDK_INT >= 37)
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val state = context.checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK)
        assertTrue(
            "permission state must be either granted or denied",
            state == android.content.pm.PackageManager.PERMISSION_GRANTED ||
                state == android.content.pm.PackageManager.PERMISSION_DENIED,
        )
        Log.i(TAG, "ACCESS_LOCAL_NETWORK state=$state")
    }

    private sealed interface CallbackOutcome {
        data object Started : CallbackOutcome
        data object Stopped : CallbackOutcome
        data class Failed(val code: Int) : CallbackOutcome
    }

    private companion object {
        const val TAG = "Android17DiscoveryProbe"
        const val SERVICE_TYPE = "_sonimesh._tcp"
        const val TIMEOUT_SECONDS = 10L
    }
}
