package com.sonimesh.opusspike

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LabCaptureAdmissionTest {
    @Test
    fun cancelledRequestsWaitForTheirMatchingResultBeforeRetry() {
        for (phase in 0..2) for (granted in listOf(false, true)) {
            val admission = LabCaptureAdmission()
            assertTrue(admission.begin(needsPermission = phase == 0, needsNotification = phase == 1))
            assertTrue(admission.cancelPending())
            assertTrue(admission.cancellationPending())
            val unrelated = when (phase) {
                0 -> admission.notificationResult(true)
                1 -> admission.projectionResult(true)
                else -> admission.permissionResult(true)
            }
            assertFalse(unrelated)
            assertFalse(admission.canBegin())
            assertFalse(admission.begin(false))
            val accepted = when (phase) {
                0 -> admission.permissionResult(granted)
                1 -> admission.notificationResult(granted)
                else -> admission.projectionResult(granted)
            }
            assertFalse(accepted)
            assertFalse(admission.cancellationPending())
            assertTrue(admission.canBegin())
            assertTrue(admission.begin(false))
        }
    }

    @Test
    fun cancellationDoesNotReplaceServiceStop() {
        val admission = LabCaptureAdmission()
        assertFalse(admission.cancelPending())
        assertTrue(admission.begin(false))
        assertTrue(admission.projectionResult(true))
        assertFalse(admission.cancelPending())
        admission.serviceStarted()
        assertFalse(admission.cancelPending())
        admission.serviceFinished()
        assertTrue(admission.canBegin())
    }

    @Test
    fun notificationPermissionIsOrderedAndDenialAllowsRetry() {
        val admission = LabCaptureAdmission()
        assertTrue(admission.begin(needsPermission = true, needsNotification = true))
        assertFalse(admission.notificationResult(true))
        assertTrue(admission.permissionResult(true))
        assertFalse(admission.projectionResult(true))
        assertTrue(admission.notificationResult(false))
        assertTrue(admission.canBegin())
        assertTrue(admission.begin(needsPermission = false, needsNotification = true))
        assertTrue(admission.notificationResult(true))
        assertTrue(admission.projectionResult(true))
        admission.launchFailed()
        assertTrue(admission.canBegin())
        assertTrue(admission.begin(needsPermission = false, needsNotification = true))
        admission.launchFailed()
        assertTrue(admission.canBegin())
    }
    @Test
    fun admitsOnlyOneFlowUntilTheServiceFinishes() {
        val admission = LabCaptureAdmission()
        assertTrue(admission.begin(needsPermission = true))
        assertFalse(admission.begin(needsPermission = false))
        assertFalse(admission.projectionResult(true))
        assertTrue(admission.permissionResult(true))
        assertFalse(admission.begin(needsPermission = false))
        assertFalse(admission.permissionResult(true))
        assertTrue(admission.projectionResult(true))
        assertFalse(admission.begin(needsPermission = false))
        admission.serviceStarted()
        assertFalse(admission.begin(needsPermission = false))
        admission.serviceFinished()
        assertTrue(admission.begin(needsPermission = false))
    }

    @Test
    fun denialAndLaunchFailureAllowAnotherRequest() {
        val admission = LabCaptureAdmission()
        assertTrue(admission.begin(needsPermission = true))
        assertTrue(admission.permissionResult(false))
        assertTrue(admission.begin(needsPermission = false))
        assertTrue(admission.projectionResult(false))
        assertTrue(admission.begin(needsPermission = false))
        assertTrue(admission.projectionResult(true))
        admission.launchFailed()
        assertTrue(admission.begin(needsPermission = true))
        admission.launchFailed()
        assertTrue(admission.canBegin())
    }
}
