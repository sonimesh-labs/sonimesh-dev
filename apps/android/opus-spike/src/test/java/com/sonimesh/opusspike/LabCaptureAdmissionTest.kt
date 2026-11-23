package com.sonimesh.opusspike

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LabCaptureAdmissionTest {
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
