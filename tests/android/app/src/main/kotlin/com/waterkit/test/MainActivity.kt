package com.waterkit.test

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat

/**
 * Reusable test activity for waterkit crates.
 * Add new test sections by extending TestSection interface.
 */
class MainActivity : AppCompatActivity() {
    
    private lateinit var logText: TextView
    
    companion object {
        init {
            System.loadLibrary("waterkit_test")
        }
    }
    
    // ===== JNI declarations - add new crate tests here =====
    
    // Permission crate
    private external fun testCheckPermission(activity: AppCompatActivity, permissionType: Int): Int
    
    // Location crate  
    private external fun testGetLocation(context: android.content.Context): DoubleArray?
    
    // ===== End JNI declarations =====
    
    private val requestLocationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        log(if (granted) "✓ Location permission granted" else "✗ Location permission denied")
    }
    
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        
        val scroll = ScrollView(this)
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
        
        // ===== Permission Tests =====
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
        
        scroll.addView(layout)
        setContentView(scroll)
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
