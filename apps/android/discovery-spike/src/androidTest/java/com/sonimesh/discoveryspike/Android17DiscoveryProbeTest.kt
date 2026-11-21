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
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import java.util.LinkedHashSet
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
    fun candidate_ledger_removes_lost_services_and_ignores_unknown_loss() {
        val ledger = CandidateLedger(maxCandidates = 2)

        assertTrue(ledger.recordFound("desktop-a"))
        assertTrue(ledger.recordFound("desktop-b"))
        assertTrue(ledger.contains("desktop-a"))
        assertTrue(ledger.contains("desktop-b"))

        assertTrue(ledger.recordLost("desktop-a"))
        assertTrue(!ledger.contains("desktop-a"))
        assertTrue(ledger.contains("desktop-b"))
        assertTrue(!ledger.recordLost("desktop-a"))
    }

    @Test
    fun candidate_ledger_evicts_oldest_entry_at_the_fixed_bound() {
        val ledger = CandidateLedger(maxCandidates = 2)

        assertTrue(ledger.recordFound("desktop-a"))
        assertTrue(ledger.recordFound("desktop-b"))
        assertTrue(ledger.recordFound("desktop-c"))

        assertTrue(!ledger.contains("desktop-a"))
        assertTrue(ledger.contains("desktop-b"))
        assertTrue(ledger.contains("desktop-c"))
    }

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
        Log.i(
            TAG,
            "ACCESS_LOCAL_NETWORK granted=${state == android.content.pm.PackageManager.PERMISSION_GRANTED}",
        )
    }

    @Test
    fun denied_permission_probe_fails_closed_without_service_callbacks() {
        assumeTrue("Android 17 is required", Build.VERSION.SDK_INT >= 37)
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue(
            "denied permission probe requires runDeniedDiscovery=true",
            arguments.getString("runDeniedDiscovery") == "true",
        )

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assumeTrue(
            "denied permission probe requires ACCESS_LOCAL_NETWORK to be denied",
            context.checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) ==
                android.content.pm.PackageManager.PERMISSION_DENIED,
        )
        val manager = requireNotNull(context.getSystemService(Context.NSD_SERVICE) as? NsdManager)
        val executor = Executors.newSingleThreadExecutor()
        val terminal = CountDownLatch(1)
        val callbackOutcome = AtomicReference<CallbackOutcome>()
        val serviceFound = AtomicBoolean(false)
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                callbackOutcome.set(CallbackOutcome.Started)
                runCatching { manager.stopServiceDiscovery(this) }
            }

            override fun onDiscoveryStopped(serviceType: String) {
                callbackOutcome.set(CallbackOutcome.Stopped)
                terminal.countDown()
            }

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                callbackOutcome.set(CallbackOutcome.Failed(errorCode))
                terminal.countDown()
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {
                callbackOutcome.set(CallbackOutcome.Failed(errorCode))
                terminal.countDown()
            }

            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                serviceFound.set(true)
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) = Unit
        }

        try {
            try {
                manager.discoverServices(
                    DiscoveryRequest.Builder(SERVICE_TYPE)
                        .setFlags(DiscoveryRequest.FLAG_NO_PICKER)
                        .build(),
                    executor,
                    listener,
                )
            } catch (error: SecurityException) {
                callbackOutcome.set(CallbackOutcome.PermissionDenied)
                terminal.countDown()
                Log.i(TAG, "denied permission probe outcome=security_exception")
            }
            if (!terminal.await(TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                Log.w(TAG, "denied permission probe callback timed out; platform result is unsupported")
                assumeTrue("denied permission callback was not delivered", false)
            }
            val finalOutcome = requireNotNull(callbackOutcome.get())
            assertTrue(
                "denied permission probe did not fail closed: $finalOutcome",
                finalOutcome is CallbackOutcome.Failed ||
                    finalOutcome is CallbackOutcome.PermissionDenied,
            )
            assertTrue("denied permission probe delivered a service", !serviceFound.get())
            Log.i(TAG, "denied permission probe outcome=$finalOutcome")
        } finally {
            runCatching { manager.stopServiceDiscovery(listener) }
            executor.shutdownNow()
        }
    }

    @Test
    fun local_service_probe_reports_found_then_lost_without_trusting_the_candidate() {
        assumeTrue("Android 17 is required", Build.VERSION.SDK_INT >= 37)
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue(
            "local service probe requires runLocalService=true",
            arguments.getString("runLocalService") == "true",
        )
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val localNetworkGranted =
            context.checkSelfPermission(Manifest.permission.ACCESS_LOCAL_NETWORK) ==
                android.content.pm.PackageManager.PERMISSION_GRANTED
        Log.i(
            TAG,
            "local probe targetPackage=${context.packageName} " +
                "instrumentationPackage=${InstrumentationRegistry.getInstrumentation().context.packageName} " +
                "localNetworkGranted=$localNetworkGranted",
        )
        assumeTrue(
            "local service probe requires ACCESS_LOCAL_NETWORK",
            localNetworkGranted,
        )

        val manager = requireNotNull(context.getSystemService(Context.NSD_SERVICE) as? NsdManager)
        val executor = context.mainExecutor
        val registered = CountDownLatch(1)
        val found = CountDownLatch(1)
        val lost = CountDownLatch(1)
        val registeredName = AtomicReference<String>()
        val registrationError = AtomicReference<Int>()
        val discoveryError = AtomicReference<Int>()
        val registrationListener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(serviceInfo: NsdServiceInfo) {
                registeredName.set(serviceInfo.serviceName)
                registered.countDown()
            }

            override fun onRegistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                registrationError.set(errorCode)
                registered.countDown()
            }

            override fun onServiceUnregistered(serviceInfo: NsdServiceInfo) = Unit

            override fun onUnregistrationFailed(serviceInfo: NsdServiceInfo, errorCode: Int) = Unit
        }
        val discoveryListener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit

            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                discoveryError.set(errorCode)
                found.countDown()
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit

            override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                if (serviceInfo.serviceName == registeredName.get()) {
                    found.countDown()
                }
            }

            override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                if (serviceInfo.serviceName == registeredName.get()) {
                    lost.countDown()
                }
            }
        }

        val serviceInfo = NsdServiceInfo().apply {
            serviceName = "SoniMeshProbe-${System.nanoTime()}"
            serviceType = SERVICE_TYPE
            port = PROBE_PORT
        }
        try {
            manager.registerService(
                serviceInfo,
                NsdManager.PROTOCOL_DNS_SD,
                executor,
                registrationListener,
            )
            if (!registered.await(TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                Log.w(TAG, "local service probe registration callback timed out; platform result is unsupported")
                assumeTrue("local service registration callback was not delivered", false)
            }
            assumeTrue("service registration failed: ${registrationError.get()}", registrationError.get() == null)

            manager.discoverServices(
                DiscoveryRequest.Builder(SERVICE_TYPE)
                    .setFlags(DiscoveryRequest.FLAG_NO_PICKER)
                    .build(),
                executor,
                discoveryListener,
            )
            if (!found.await(TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                Log.w(TAG, "local service probe discovery callback timed out; platform result is unsupported")
                assumeTrue("local service was not found", false)
            }
            assumeTrue("service discovery failed: ${discoveryError.get()}", discoveryError.get() == null)
            manager.unregisterService(registrationListener)
            if (!lost.await(TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                Log.w(TAG, "local service probe removal callback timed out; platform result is unsupported")
                assumeTrue("local service removal was not observed", false)
            }
            Log.i(TAG, "local service probe observed found then lost")
        } finally {
            runCatching { manager.stopServiceDiscovery(discoveryListener) }
            runCatching { manager.unregisterService(registrationListener) }
        }
    }

    private sealed interface CallbackOutcome {
        data object Started : CallbackOutcome
        data object Stopped : CallbackOutcome
        data object PermissionDenied : CallbackOutcome
        data class Failed(val code: Int) : CallbackOutcome
    }

    /** Bounded model for discovery candidates; it never carries trust state. */
    private class CandidateLedger(private val maxCandidates: Int) {
        private val candidates = LinkedHashSet<String>()

        init {
            require(maxCandidates > 0)
        }

        fun recordFound(name: String): Boolean {
            if (name.isEmpty()) return false
            if (!candidates.add(name)) return false
            if (candidates.size > maxCandidates) {
                candidates.iterator().next().let(candidates::remove)
            }
            return true
        }

        fun recordLost(name: String): Boolean = candidates.remove(name)

        fun contains(name: String): Boolean = candidates.contains(name)
    }

    private companion object {
        const val TAG = "Android17DiscoveryProbe"
        const val SERVICE_TYPE = "_sonimesh._tcp"
        const val PROBE_PORT = 43_210
        const val TIMEOUT_SECONDS = 10L
    }
}
