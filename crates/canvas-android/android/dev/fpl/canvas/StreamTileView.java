package dev.fpl.canvas;

import android.content.Context;
import android.graphics.Color;
import android.media.MediaPlayer;
import android.net.Uri;
import android.view.Gravity;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.VideoView;
import org.json.JSONObject;

/** An independently owned platform decoder. Layout changes never restart it. */
final class StreamTileView extends FrameLayout {
    private final VideoView video;
    private final TextView status;
    private final Button mute;
    private MediaPlayer player;
    private final String url;
    private boolean muted = true;
    private boolean active = true;

    StreamTileView(Context context, String title, String source, JSONObject palette) throws Exception {
        super(context);
        url = source;
        setBackgroundColor(Color.BLACK);
        video = new VideoView(context);
        video.setZOrderMediaOverlay(true);
        addView(video, new LayoutParams(-1, -1, Gravity.CENTER));
        status = new TextView(context);
        status.setText("Connecting…");
        status.setTextColor(Color.parseColor(palette.getString("text")));
        status.setGravity(Gravity.CENTER);
        addView(status, new LayoutParams(-1, -1));
        mute = new Button(context);
        mute.setAllCaps(false);
        mute.setTextSize(12);
        mute.setTextColor(Color.parseColor(palette.getString("text")));
        mute.setBackgroundTintList(android.content.res.ColorStateList.valueOf(Color.parseColor(palette.getString("surface"))));
        mute.setText(title + " · Unmute");
        mute.setOnClickListener(v -> {
            muted = !muted;
            if (player != null) player.setVolume(muted ? 0 : 1, muted ? 0 : 1);
            mute.setText(title + (muted ? " · Unmute" : " · Mute"));
        });
        addView(mute, new LayoutParams(-2, -2, Gravity.TOP | Gravity.RIGHT));
        video.setOnPreparedListener(p -> {
            player = p;
            p.setVolume(muted ? 0 : 1, muted ? 0 : 1);
            status.setVisibility(View.GONE);
            if (active) video.start();
        });
        video.setOnErrorListener((p, what, extra) -> {
            status.setText("Playback failed (" + what + ", " + extra + ")");
            status.setVisibility(View.VISIBLE);
            return true;
        });
        video.setOnCompletionListener(p -> { status.setText("Stream ended"); status.setVisibility(View.VISIBLE); });
        video.setVideoURI(Uri.parse(url));
    }

    boolean matches(String source) { return url.equals(source); }
    Button muteControl() { return mute; }
    void dispose() { active = false; player = null; video.stopPlayback(); }
}
