package waterkit.media

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.graphics.BitmapFactory
import java.net.URL
import kotlin.concurrent.thread

object MediaSessionHelper {
    private var mediaSession: MediaSession? = null
    private var audioManager: AudioManager? = null
    private var audioFocusRequest: AudioFocusRequest? = null
    private var context: Context? = null
    
    @JvmStatic
    fun createSession(ctx: Context) {
        context = ctx.applicationContext
        audioManager = ctx.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        
        mediaSession = MediaSession(ctx, "WaterKitMedia").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() {
                    // Callback to Rust would go here
                }
                
                override fun onPause() {
                    // Callback to Rust would go here
                }
                
                override fun onStop() {
                    // Callback to Rust would go here
                }
                
                override fun onSkipToNext() {
                    // Callback to Rust would go here
                }
                
                override fun onSkipToPrevious() {
                    // Callback to Rust would go here
                }
                
                override fun onSeekTo(pos: Long) {
                    // Callback to Rust would go here
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
                PlaybackState.ACTION_SKIP_TO_NEXT or
                PlaybackState.ACTION_SKIP_TO_PREVIOUS or
                PlaybackState.ACTION_SEEK_TO
        
        val playbackState = PlaybackState.Builder()
            .setState(state, if (positionMs >= 0) positionMs else PlaybackState.PLAYBACK_POSITION_UNKNOWN, speed)
            .setActions(actions)
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
                .build()
            audioFocusRequest = focusRequest
            am.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        } else {
            @Suppress("DEPRECATION")
            am.requestAudioFocus(
                null,
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
            am.abandonAudioFocus(null)
        }
    }
    
    @JvmStatic
    fun clearSession() {
        mediaSession?.isActive = false
        mediaSession?.release()
        mediaSession = null
    }
}
