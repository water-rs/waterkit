package waterkit.media

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.graphics.BitmapFactory
import java.net.URL
import java.util.concurrent.ConcurrentLinkedQueue
import kotlin.concurrent.thread

object MediaSessionHelper {
    private var mediaSession: MediaSession? = null
    private var applicationContext: Context? = null
    private var audioManager: AudioManager? = null
    private var audioFocusRequest: AudioFocusRequest? = null
    private val commandQueue = ConcurrentLinkedQueue<String>()
    private var noisyReceiver: BroadcastReceiver? = null
    private val audioFocusChangeListener = AudioManager.OnAudioFocusChangeListener { focusChange ->
        when (focusChange) {
            AudioManager.AUDIOFOCUS_GAIN -> commandQueue.add("audio_focus_gained")
            AudioManager.AUDIOFOCUS_LOSS -> commandQueue.add("audio_focus_lost")
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> commandQueue.add("audio_focus_lost_transient")
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> commandQueue.add("audio_focus_lost_duck")
        }
    }
    
    @JvmStatic
    fun createSession(ctx: Context) {
        val appContext = ctx.applicationContext
        applicationContext = appContext
        audioManager = appContext.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        registerNoisyReceiver(appContext)
        
        mediaSession = MediaSession(ctx, "WaterKitMedia").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() {
                    commandQueue.add("play")
                }
                
                override fun onPause() {
                    commandQueue.add("pause")
                }
                
                override fun onStop() {
                    commandQueue.add("stop")
                }
                
                override fun onSkipToNext() {
                    commandQueue.add("next")
                }
                
                override fun onSkipToPrevious() {
                    commandQueue.add("previous")
                }
                
                override fun onSeekTo(pos: Long) {
                    commandQueue.add("seek:$pos")
                }
            })
            isActive = true
        }
    }
    
    @JvmStatic
    fun setMetadata(title: String, artist: String, album: String, artworkUrl: String, durationMs: Long) {
        val builder = MediaMetadata.Builder()
        
        if (title.isNotEmpty()) {
            builder.putString(MediaMetadata.METADATA_KEY_TITLE, title)
        }
        if (artist.isNotEmpty()) {
            builder.putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
        }
        if (album.isNotEmpty()) {
            builder.putString(MediaMetadata.METADATA_KEY_ALBUM, album)
        }
        if (durationMs >= 0) {
            builder.putLong(MediaMetadata.METADATA_KEY_DURATION, durationMs)
        }
        
        // Load artwork from URL in background
        if (artworkUrl.isNotEmpty()) {
            thread {
                try {
                    val url = URL(artworkUrl)
                    val bitmap = BitmapFactory.decodeStream(url.openStream())
                    if (bitmap != null) {
                        val updatedMetadata = MediaMetadata.Builder(mediaSession?.controller?.metadata)
                            .putBitmap(MediaMetadata.METADATA_KEY_ART, bitmap)
                            .build()
                        mediaSession?.setMetadata(updatedMetadata)
                    }
                } catch (e: Exception) {
                    // Ignore artwork loading errors
                }
            }
        }
        
        mediaSession?.setMetadata(builder.build())
    }
    
    @JvmStatic
    fun setPlaybackState(status: Int, positionMs: Long, speed: Float) {
        setPlaybackState(status, positionMs, speed, true, true)
    }

    @JvmStatic
    fun setPlaybackState(
        status: Int,
        positionMs: Long,
        speed: Float,
        nextEnabled: Boolean,
        previousEnabled: Boolean
    ) {
        val state = when (status) {
            0 -> PlaybackState.STATE_STOPPED
            1 -> PlaybackState.STATE_PAUSED
            2 -> PlaybackState.STATE_PLAYING
            else -> PlaybackState.STATE_NONE
        }
        
        val actions = PlaybackState.ACTION_PLAY or
                PlaybackState.ACTION_PAUSE or
                PlaybackState.ACTION_PLAY_PAUSE or
                PlaybackState.ACTION_STOP or
                PlaybackState.ACTION_SEEK_TO
        val queueActions = (if (nextEnabled) PlaybackState.ACTION_SKIP_TO_NEXT else 0L) or
                (if (previousEnabled) PlaybackState.ACTION_SKIP_TO_PREVIOUS else 0L)
        
        val playbackState = PlaybackState.Builder()
            .setState(state, if (positionMs >= 0) positionMs else PlaybackState.PLAYBACK_POSITION_UNKNOWN, speed)
            .setActions(actions or queueActions)
            .build()
        
        mediaSession?.setPlaybackState(playbackState)
    }
    
    @JvmStatic
    fun requestAudioFocus(): Boolean {
        val am = audioManager ?: return false
        
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build()
                )
                .setOnAudioFocusChangeListener(audioFocusChangeListener)
                .build()
            audioFocusRequest = focusRequest
            am.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        } else {
            @Suppress("DEPRECATION")
            am.requestAudioFocus(
                audioFocusChangeListener,
                AudioManager.STREAM_MUSIC,
                AudioManager.AUDIOFOCUS_GAIN
            ) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
    }
    
    @JvmStatic
    fun abandonAudioFocus() {
        val am = audioManager ?: return
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
        } else {
            @Suppress("DEPRECATION")
            am.abandonAudioFocus(audioFocusChangeListener)
        }
    }
    
    @JvmStatic
    fun clearSession() {
        abandonAudioFocus()
        mediaSession?.isActive = false
        mediaSession?.release()
        mediaSession = null
        unregisterNoisyReceiver()
        applicationContext = null
        commandQueue.clear()
    }

    @JvmStatic
    fun pollCommand(): String? {
        return commandQueue.poll()
    }

    private fun registerNoisyReceiver(context: Context) {
        if (noisyReceiver != null) {
            return
        }

        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                    commandQueue.add("audio_becoming_noisy")
                }
            }
        }
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("DEPRECATION")
            context.registerReceiver(receiver, filter)
        }
        noisyReceiver = receiver
    }

    private fun unregisterNoisyReceiver() {
        val receiver = noisyReceiver ?: return
        val context = applicationContext ?: return
        context.unregisterReceiver(receiver)
        noisyReceiver = null
    }
}
