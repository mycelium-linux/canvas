package dev.fpl.canvas;

import android.app.Activity;
import android.content.Intent;
import android.media.AudioManager;
import android.media.MediaPlayer;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.VideoView;
import android.widget.Button;

/** Optional platform-only shim: no discovery, credentials, or workspace policy. */
public final class TvActivity extends Activity {
    static { System.loadLibrary("canvas_android"); }
    private static native String nativeValidate(String url);
    private static native float nativeSetMuted(boolean muted);
    private static native boolean nativeIsMuted();
    private static native int[] nativeVideoFrame(int width, int height, boolean windowed);
    private VideoView video;
    private TextView status;
    private FrameLayout frame;
    private Button muteButton;
    private MediaPlayer preparedPlayer;
    private boolean resumed;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        setVolumeControlStream(AudioManager.STREAM_MUSIC);
        getWindow().getDecorView().setSystemUiVisibility(
            View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
            | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        frame = new FrameLayout(this);
        frame.setBackgroundColor(0xff000000);
        video = new VideoView(this);
        FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER);
        frame.addView(video, geometry);
        status = new TextView(this);
        status.setTextColor(0xffffffff);
        status.setTextSize(24);
        status.setGravity(Gravity.CENTER);
        frame.addView(status, new FrameLayout.LayoutParams(-1, -1));
        muteButton = new Button(this);
        muteButton.setTextSize(16);
        muteButton.setOnClickListener(v -> setSurfaceMuted(!nativeIsMuted()));
        frame.addView(muteButton, new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.LEFT));
        setSurfaceMuted(getIntent().getBooleanExtra("muted", false));
        setContentView(frame);
        frame.addOnLayoutChangeListener((v, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) {
                applySurfaceGeometry();
            }
        });
        video.setOnPreparedListener(player -> {
            preparedPlayer = player;
            setSurfaceMuted(nativeIsMuted());
            status.setVisibility(View.GONE);
            if (resumed) video.start();
        });
        video.setOnErrorListener((player, what, extra) -> {
            showStatus("Playback failed (" + what + ", " + extra + ")");
            return true;
        });
        video.setOnCompletionListener(player -> showStatus("Stream ended"));
        applyIntent(getIntent());
    }

    private void showStatus(String text) {
        status.setText(text);
        status.setVisibility(View.VISIBLE);
    }

    private void applyIntent(Intent intent) {
        applySurfaceGeometry();
        preparedPlayer = null;
        video.stopPlayback();
        if (intent.hasExtra("muted")) setSurfaceMuted(intent.getBooleanExtra("muted", false));
        String url = intent.getStringExtra("stream_url");
        if (url == null) { showStatus("fungOS Fire TV — select a stream"); return; }
        String error = nativeValidate(url);
        if (error == null || !error.isEmpty()) {
            showStatus(error == null ? "Command validation failed" : error);
            return;
        }
        showStatus("Connecting…");
        video.setVideoURI(Uri.parse(url));
    }

    private void setSurfaceMuted(boolean muted) {
        float gain = nativeSetMuted(muted);
        if (preparedPlayer != null) preparedPlayer.setVolume(gain, gain);
        muteButton.setText(muted ? "KOMO · Unmute" : "KOMO · Mute");
        muteButton.setContentDescription(muted ? "Unmute KOMO audio" : "Mute KOMO audio");
    }

    // Rust's shared Canvas layout solver owns geometry. Android only applies
    // pixel bounds and preserves decoded video aspect ratio within them.
    private void applySurfaceGeometry() {
        boolean windowed = getIntent().getBooleanExtra("windowed", false);
        int width = frame.getWidth();
        int height = frame.getHeight();
        if (width <= 0 || height <= 0) return;
        int[] bounds = nativeVideoFrame(width, height, windowed);
        if (bounds == null || bounds.length != 4) {
            showStatus("Surface layout failed");
            return;
        }
        FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(bounds[2], bounds[3], Gravity.TOP | Gravity.LEFT);
        geometry.leftMargin = bounds[0];
        geometry.topMargin = bounds[1];
        video.setLayoutParams(geometry);
        FrameLayout.LayoutParams chrome = new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.LEFT);
        chrome.leftMargin = width * 4 / 100;
        chrome.topMargin = height * 4 / 100;
        muteButton.setLayoutParams(chrome);
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        applyIntent(intent);
    }

    @Override protected void onResume() {
        super.onResume();
        resumed = true;
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        video.start();
    }

    @Override protected void onPause() {
        resumed = false;
        preparedPlayer = null;
        video.stopPlayback(); // Release tuner when the app is no longer visible.
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        super.onPause();
    }

    @Override public boolean onKeyDown(int key, KeyEvent event) {
        if (key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE) {
            if (video.isPlaying()) video.pause(); else video.start();
            return true;
        }
        return super.onKeyDown(key, event);
    }
}
