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
import android.widget.LinearLayout;
import android.widget.HorizontalScrollView;
import android.widget.TextClock;
import android.os.Handler;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.StateListDrawable;
import android.content.res.ColorStateList;
import org.json.JSONObject;
import org.json.JSONArray;
import java.util.ArrayList;

/** Optional platform-only shim: no discovery, credentials, or workspace policy. */
public final class TvActivity extends Activity {
    static { System.loadLibrary("canvas_android"); }
    private static native String nativeValidate(String url);
    private static native float nativeSetMuted(boolean muted);
    private static native boolean nativeIsMuted();
    private static native int[] nativeVideoFrame(int width, int height, boolean windowed);
    private static native String nativeStartControl(String directory);
    private static native String nativeProjection(int width, int height);
    private static native String nativeSelectWorkspace(String project);
    private VideoView video;
    private TextView status;
    private FrameLayout frame;
    private Button muteButton;
    private MediaPlayer preparedPlayer;
    private boolean resumed;
    private LinearLayout selector;
    private final Handler updates = new Handler();
    private String lastProjection = "";
    private String currentUrl = "";
    private String surfaceTitle = "KOMO";
    private boolean workspaceMode;
    private final ArrayList<View> widgets = new ArrayList<>();
    private final Runnable refresh = new Runnable() {
        @Override public void run() {
            if (!resumed) return;
            refreshWorkspace();
            updates.postDelayed(this, 250);
        }
    };

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
        selector = new LinearLayout(this);
        selector.setOrientation(LinearLayout.HORIZONTAL);
        HorizontalScrollView scroller = new HorizontalScrollView(this);
        scroller.addView(selector);
        FrameLayout.LayoutParams bar = new FrameLayout.LayoutParams(-1, barHeight(), Gravity.TOP);
        frame.addView(scroller, bar);
        String controlError = nativeStartControl(getFilesDir().getAbsolutePath());
        if (controlError == null || !controlError.isEmpty()) {
            showStatus("Control unavailable: " + controlError);
        }
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
        if (workspaceMode) return;
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
        currentUrl = url;
        video.setVideoURI(Uri.parse(url));
    }

    private void setSurfaceMuted(boolean muted) {
        float gain = nativeSetMuted(muted);
        if (preparedPlayer != null) preparedPlayer.setVolume(gain, gain);
        muteButton.setText(surfaceTitle + (muted ? " · Unmute" : " · Mute"));
        muteButton.setContentDescription((muted ? "Unmute " : "Mute ") + surfaceTitle + " audio");
    }

    // Rust's shared Canvas layout solver owns geometry. Android only applies
    // pixel bounds and preserves decoded video aspect ratio within them.
    private void applySurfaceGeometry() {
        if (workspaceMode) { lastProjection = ""; return; }
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
        lastProjection = "";
        if (workspaceMode) currentUrl = "";
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        video.start();
        updates.post(refresh);
    }

    @Override protected void onPause() {
        resumed = false;
        updates.removeCallbacks(refresh);
        preparedPlayer = null;
        video.stopPlayback(); // Release tuner when the app is no longer visible.
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        super.onPause();
    }

    @Override public boolean onKeyDown(int key, KeyEvent event) {
        if (key == KeyEvent.KEYCODE_MENU && selector.getChildCount() > 0) {
            selector.getChildAt(0).requestFocus();
            return true;
        }
        if (key == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE) {
            if (video.isPlaying()) video.pause(); else video.start();
            return true;
        }
        return super.onKeyDown(key, event);
    }

    private void styleControl(Button button, JSONObject palette, boolean selected) throws Exception {
        int primary = Color.parseColor(palette.getString("primary"));
        int text = Color.parseColor(palette.getString("text"));
        int muted = Color.parseColor(palette.getString("text_muted"));
        int surface = Color.parseColor(palette.getString("surface"));
        int border = Color.parseColor(palette.getString("border"));
        float density = getResources().getDisplayMetrics().density;
        StateListDrawable states = new StateListDrawable();
        for (int i = 0; i < 3; i++) {
            GradientDrawable shape = new GradientDrawable();
            shape.setColor(surface);
            shape.setCornerRadius(8 * density);
            shape.setStroke((int)((i < 2 ? 2 : 1) * density), i < 2 || selected ? primary : border);
            states.addState(i == 0 ? new int[]{android.R.attr.state_pressed}
                : i == 1 ? new int[]{android.R.attr.state_focused} : new int[]{}, shape);
        }
        button.setBackgroundTintList(null);
        button.setBackground(states);
        button.setTextColor(new ColorStateList(new int[][]{
            {android.R.attr.state_focused}, {}}, new int[]{primary, selected ? text : muted}));
        button.setAllCaps(false);
        button.setElevation(0);
        button.setStateListAnimator(null);
        button.setPadding((int)(18 * density), 0, (int)(18 * density), 0);
    }

    private void refreshWorkspace() {
        int width = frame.getWidth(), height = frame.getHeight() - barHeight();
        if (width <= 0 || height <= 0) return;
        String raw = nativeProjection(width, height);
        if (raw == null || raw.equals(lastProjection)) return;
        try {
            JSONObject projection = new JSONObject(raw);
            if (projection.has("error")) { showStatus(projection.getString("error")); return; }
            if (projection.isNull("document")) return;
            workspaceMode = true;
            JSONObject palette = projection.getJSONObject("palette");
            frame.setBackgroundColor(Color.parseColor(palette.getString("background")));
            selector.setBackgroundColor(Color.parseColor(palette.getString("surface")));
            styleControl(muteButton, palette, false);
            JSONObject catalog = projection.getJSONObject("catalog");
            JSONArray projects = catalog.getJSONArray("projects");
            String focusedProject = "";
            View focused = selector.getFocusedChild();
            if (focused != null && focused.getTag() != null) focusedProject = focused.getTag().toString();
            selector.removeAllViews();
            for (int i = 0; i < projects.length(); i++) {
                JSONObject project = projects.getJSONObject(i);
                final String projectId = project.getString("id");
                Button button = new Button(this);
                button.setTag(projectId);
                button.setText((projectId.equals(catalog.getString("selected_project")) ? "● " : "") + project.getString("name"));
                button.setTextSize(14);
                styleControl(button, palette, projectId.equals(catalog.getString("selected_project")));
                button.setOnClickListener(v -> {
                    String error = nativeSelectWorkspace(projectId);
                    if (error == null || !error.isEmpty()) showStatus("Switch failed: " + error);
                    else { lastProjection = ""; refreshWorkspace(); }
                });
                LinearLayout.LayoutParams item = new LinearLayout.LayoutParams(-2, barHeight() - 24);
                item.setMargins(12, 12, 0, 12);
                selector.addView(button, item);
                if (projectId.equals(focusedProject)) button.requestFocus();
            }
            for (View widget : widgets) frame.removeView(widget);
            widgets.clear();
            JSONArray tiles = projection.getJSONArray("tiles");
            boolean hasVideo = false;
            for (int i = 0; i < tiles.length(); i++) {
                JSONObject tile = tiles.getJSONObject(i), surface = tile.getJSONObject("surface");
                JSONObject content = surface.getJSONObject("content");
                JSONArray bounds = tile.getJSONArray("bounds");
                int x = bounds.getInt(0), y = bounds.getInt(1) + barHeight();
                int w = bounds.getInt(2), h = bounds.getInt(3);
                FrameLayout.LayoutParams geometry = new FrameLayout.LayoutParams(w, h, Gravity.TOP | Gravity.LEFT);
                geometry.leftMargin = x; geometry.topMargin = y;
                if (content.getString("kind").equals("stream") && !hasVideo) {
                    String url = content.getString("url");
                    String error = nativeValidate(url);
                    if (error == null || !error.isEmpty()) throw new IllegalArgumentException("Stream rejected: " + error);
                    hasVideo = true;
                    video.setVisibility(View.VISIBLE);
                    video.setLayoutParams(geometry);
                    surfaceTitle = surface.getString("title");
                    setSurfaceMuted(nativeIsMuted());
                    if (!url.equals(currentUrl)) {
                        preparedPlayer = null;
                        video.stopPlayback();
                        currentUrl = url;
                        showStatus("Connecting…");
                        video.setVideoURI(Uri.parse(url));
                    }
                } else {
                    TextView widget;
                    String kind = content.getString("kind");
                    if (kind.equals("widget") && content.getJSONObject("spec").getString("kind").equals("clock")) {
                        TextClock clock = new TextClock(this);
                        clock.setFormat12Hour("h:mm:ss a"); clock.setFormat24Hour("HH:mm:ss");
                        widget = clock;
                    } else {
                        widget = new TextView(this);
                        String label = surface.getString("title");
                        if (kind.equals("scene")) {
                            JSONArray nodes = content.getJSONObject("scene").getJSONArray("nodes");
                            for (int n = 0; n < nodes.length(); n++) {
                                JSONObject node = nodes.getJSONObject(n).getJSONObject("content");
                                if (node.getString("kind").equals("text")) label += "\n" + node.getString("text");
                            }
                        } else label += "\nUnsupported on TV: " + kind;
                        widget.setText(label);
                    }
                    widget.setTextSize(widget instanceof TextClock ? 28 : 18);
                    widget.setTextColor(Color.parseColor(palette.getString("text")));
                    widget.setBackgroundColor(Color.parseColor(palette.getString("surface")));
                    widget.setGravity(Gravity.CENTER);
                    widget.setPadding(20, 20, 20, 20);
                    frame.addView(widget, geometry); widgets.add(widget);
                }
            }
            if (!hasVideo) {
                video.stopPlayback(); preparedPlayer = null; currentUrl = "";
                video.setVisibility(View.INVISIBLE); muteButton.setVisibility(View.GONE);
                status.setVisibility(View.GONE);
            } else {
                muteButton.setVisibility(View.VISIBLE);
                FrameLayout.LayoutParams chrome = new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.RIGHT);
                chrome.topMargin = barHeight(); muteButton.setLayoutParams(chrome);
                muteButton.bringToFront();
                if (preparedPlayer != null) status.setVisibility(View.GONE);
            }
            selector.getParent().requestLayout();
            lastProjection = raw;
        } catch (Exception error) { showStatus("Workspace presentation failed: " + error.getMessage()); }
    }

    private int barHeight() {
        return Math.round(56 * getResources().getDisplayMetrics().density);
    }
}
