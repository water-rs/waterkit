package com.waterkit.test

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import org.json.JSONObject
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/**
 * Reusable test activity for waterkit crates.
 * Add new test sections by extending TestSection interface.
 */
class MainActivity : AppCompatActivity() {
    
    private lateinit var logText: TextView
    private var pendingNativeTest = false
    private var pendingSmsDelivery = false
    private var pendingInteractiveTest = false
    private var nativeTestRunning = false
    private var manualOtpRunning = false
    
    companion object {
        private const val REPORT_FILE_NAME = "waterkit-test-report.json"
        private const val REPORT_TEMP_FILE_NAME = "waterkit-test-report.json.tmp"

        init {
            System.loadLibrary("waterkit_test_android")
        }
    }
    
    // ===== JNI declarations - add new crate tests here =====
    
    // Permission crate
    private external fun testCheckPermission(activity: AppCompatActivity, permissionType: Int): Int
    
    // Location crate  
    private external fun testGetLocation(context: android.content.Context): DoubleArray?
    
    // Generic runner
    private external fun runTest(activity: AppCompatActivity)
    private external fun runTestReport(activity: AppCompatActivity, smsDelivery: Boolean, interactive: Boolean): String
    private external fun testOtpAddressed(activity: AppCompatActivity)
    private external fun testOtpConsent(activity: AppCompatActivity)
    
    // ===== End JNI declarations =====
    
    private val requestLocationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        log(if (granted) "✓ Location permission granted" else "✗ Location permission denied")
    }
    
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        
        val scroll = ScrollView(this)
        applyEdgeToEdgeInsets(scroll)
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 24, 24, 24)
        }
        
        // Header
        layout.addView(TextView(this).apply {
            text = "Waterkit Test Framework"
            textSize = 24f
            setPadding(0, 0, 0, 16)
        })
        
        // Log output
        logText = TextView(this).apply {
            text = "Ready. Tap a test button.\n"
            textSize = 12f
            setBackgroundColor(0xFF1E1E1E.toInt())
            setTextColor(0xFF00FF00.toInt())
            setPadding(16, 16, 16, 16)
        }
        layout.addView(logText)
        
        // Generic Native Test
        layout.addView(testButton("Run Generic Native Test") {
            runNativeTest(false)
        })

        // Permission Tests
        layout.addView(sectionHeader("Permission Crate"))
        
        layout.addView(testButton("Request Location Permission") {
            requestLocationPermission.launch(Manifest.permission.ACCESS_FINE_LOCATION)
        })
        
        layout.addView(testButton("Check Location Permission (Native)") {
            val result = testCheckPermission(this, 0) // 0 = Location
            log("Permission status: ${statusName(result)}")
        })
        
        // ===== Location Tests =====
        layout.addView(sectionHeader("Location Crate"))
        
        layout.addView(testButton("Get Current Location (Native)") {
            if (!hasPermission(Manifest.permission.ACCESS_FINE_LOCATION)) {
                log("✗ Location permission not granted")
                return@testButton
            }
            
            val result = testGetLocation(this)
            if (result != null && result.isNotEmpty() && result[0] > 0.5) {
                log("✓ Location: ${result[1]}, ${result[2]}")
                log("  Altitude: ${result[3]}m, Accuracy: ${result[4]}m")
            } else {
                log("✗ Location not available")
            }
        })
        
        // ===== Camera Tests =====
        layout.addView(sectionHeader("Camera Crate"))
        
        layout.addView(testButton("List Cameras") {
            log("Listing cameras...")
            // TODO: Expose list_cameras via JNI if needed, or rely on runTest
            // For now, let's assume runTest covers it or add specific JNI calls later.
        })
        
        layout.addView(testButton("Start Camera Preview") {
            if (!hasPermission(Manifest.permission.CAMERA)) {
                requestPermissions(arrayOf(Manifest.permission.CAMERA), 1001)
                return@testButton
            }
            log("Starting camera preview...")
            // Invoking Rust via runTest for now as specific JNI bindings for UI interaction aren't fully exposed in MainActivity yet.
            // Ideally we'd have `testStartCamera()` JNI.
        })

        // ===== Video Tests =====
        layout.addView(sectionHeader("Video Crate"))
        
        layout.addView(testButton("Play Test Video") {
            log("Starting video playback check...")
            // Invoking Rust via runTest
        })

        layout.addView(sectionHeader("OTP Crate"))
        layout.addView(testButton("Start addressed SMS request") {
            startManualOtp("addressed") { testOtpAddressed(this) }
        })
        layout.addView(testButton("Start SMS consent request") {
            startManualOtp("consent") { testOtpConsent(this) }
        })
        
        scroll.addView(layout)
        setContentView(scroll)

        // The harness cold-starts this activity with `--ez run_test true`, and a
        // cold start never reaches `onNewIntent`. Without this the flag is never
        // armed, `onWindowFocusChanged` finds nothing pending, and the run sits
        // idle until the harness gives up.
        checkIntent(intent)
    }

    private fun applyEdgeToEdgeInsets(scroll: ScrollView) {
        if (Build.VERSION.SDK_INT < 35) return

        ViewCompat.setOnApplyWindowInsetsListener(scroll) { view, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            view.setPadding(0, systemBars.top, 0, systemBars.bottom)
            insets
        }
    }

    override fun onPostResume() {
        super.onPostResume()
        checkIntent(intent)
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        checkIntent(intent)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && pendingNativeTest) {
            pendingNativeTest = false
            val smsDelivery = pendingSmsDelivery
            pendingSmsDelivery = false
            val interactive = pendingInteractiveTest
            pendingInteractiveTest = false
            runNativeTest(smsDelivery, interactive)
        }
    }

    private fun checkIntent(intent: android.content.Intent) {
        if (intent.getBooleanExtra("run_test", false)) {
            intent.removeExtra("run_test")
            pendingSmsDelivery = intent.getBooleanExtra("sms_delivery", false)
            intent.removeExtra("sms_delivery")
            pendingInteractiveTest = intent.getBooleanExtra("interactive", false)
            intent.removeExtra("interactive")
            // The runner wakes the device just before launch. From here until
            // the report is written this window keeps the screen on, so the
            // screen timeout cannot take focus away mid-run. The flag belongs
            // to this window alone and changes no device setting.
            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            pendingNativeTest = true
            runPendingNativeTest()
        }
    }

    private fun runPendingNativeTest() {
        if (!pendingNativeTest || !hasWindowFocus()) return
        pendingNativeTest = false
        val smsDelivery = pendingSmsDelivery
        pendingSmsDelivery = false
        val interactive = pendingInteractiveTest
        pendingInteractiveTest = false
        runNativeTest(smsDelivery, interactive)
    }

    private fun runNativeTest(smsDelivery: Boolean = false, interactive: Boolean = false) {
        if (nativeTestRunning || manualOtpRunning) {
            log("Native test not started: another native OTP/test operation is active")
            return
        }
        nativeTestRunning = true
        log("Running native test...")
        android.util.Log.i("waterkit", "Native test started with window focus")
        // Native clipboard cases write synthetic clips; hold the user's
        // original primary clip inside the process and hand it back after
        // the run so a real device's clipboard survives the harness. The
        // clip's contents are never read, logged, or exported.
        val savedClip = snapshotPrimaryClip()
        Thread {
            var restoreFailure: Throwable? = null
            val report = try {
                runTestReport(this, smsDelivery, interactive)
            } finally {
                try {
                    restorePrimaryClip(savedClip)
                } catch (t: Throwable) {
                    restoreFailure = t
                    android.util.Log.e("waterkit", "primaryClip restore failed after native test", t)
                }
            }
            // Completion is only reported once the user's clip has been
            // restored (or the restore failure surfaced) — never while
            // synthetic clips could still be on the device clipboard. A
            // failed restore lands in the structured report so the result
            // cannot read as a clean pass.
            val failure = restoreFailure
            val finalReport = if (failure == null) {
                report
            } else {
                appendRestoreFailureCase(report, failure)
            }
            writeReport(finalReport)
            runOnUiThread {
                nativeTestRunning = false
                window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                if (failure == null) {
                    log("Native test report written")
                } else {
                    log("Native test done; clipboard restore FAILED (${failure.javaClass.simpleName})")
                }
            }
        }.start()
    }

    private fun startManualOtp(mode: String, start: () -> Unit) {
        if (pendingNativeTest || nativeTestRunning || manualOtpRunning) {
            log("OTP request not started: do not run manual OTP and runTestReport concurrently")
            return
        }
        manualOtpRunning = true
        log("Starting manual OTP $mode request...")
        try {
            start()
        } catch (error: Exception) {
            manualOtpRunning = false
            log("waterkit-otp error=${error.message}")
        }
    }

    fun logFromNative(message: String) {
        runOnUiThread { log(message) }
    }

    fun finishOtpRequest() {
        runOnUiThread { manualOtpRunning = false }
    }

    private fun appendRestoreFailureCase(report: String, failure: Throwable): String {
        val json = JSONObject(report)
        val cases = json.getJSONArray("cases")
        val case = JSONObject()
        case.put("name", "clipboard.restore_primary_clip")
        case.put("status", "failed")
        case.put("message", "primaryClip restore failed: ${failure.javaClass.simpleName}")
        cases.put(case)
        return json.toString(2)
    }

    private fun snapshotPrimaryClip(): ClipData? {
        val clipboard = getSystemService(CLIPBOARD_SERVICE) as? ClipboardManager
            ?: error("ClipboardManager unavailable — cannot preserve the device clipboard")
        return clipboard.primaryClip
    }

    private fun restorePrimaryClip(savedClip: ClipData?) {
        val clipboard = getSystemService(CLIPBOARD_SERVICE) as? ClipboardManager
            ?: error("ClipboardManager unavailable — cannot restore the device clipboard")
        if (savedClip != null) {
            clipboard.setPrimaryClip(savedClip)
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            clipboard.clearPrimaryClip()
        } else {
            clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
        }
    }

    private fun writeReport(report: String) {
        val reportFile = File(filesDir, REPORT_FILE_NAME)
        val tempReportFile = File(filesDir, REPORT_TEMP_FILE_NAME)
        tempReportFile.writeText(report)
        Files.move(
            tempReportFile.toPath(),
            reportFile.toPath(),
            StandardCopyOption.REPLACE_EXISTING,
            StandardCopyOption.ATOMIC_MOVE,
        )
        android.util.Log.i("waterkit-report", report)
    }
    
    private fun sectionHeader(title: String) = TextView(this).apply {
        text = "─── $title ───"
        textSize = 16f
        setPadding(0, 24, 0, 8)
    }
    
    private fun testButton(label: String, onClick: () -> Unit) = Button(this).apply {
        text = label
        setOnClickListener { 
            try {
                onClick()
            } catch (e: Exception) {
                log("✗ Error: ${e.message}")
            }
        }
    }
    
    private fun log(message: String) {
        logText.append("$message\n")
    }
    
    private fun hasPermission(permission: String) = 
        ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED
    
    private fun statusName(status: Int) = when (status) {
        0 -> "NotDetermined"
        1 -> "Restricted"
        2 -> "Denied"
        3 -> "Granted"
        else -> "Error($status)"
    }
}