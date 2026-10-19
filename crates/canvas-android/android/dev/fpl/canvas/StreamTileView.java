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
    private VideoView video;
    private MattedVideoView matted;
    private final String matteIdentity;
    private final TextView status;
    private final Button mute;
    private MediaPlayer player;
    private final String url;
    private boolean muted = true;
    private boolean active = true;
    private int audioAvailability=DecoderAudio.UNKNOWN;

    StreamTileView(Context context, String title, String source, JSONObject palette, JSONObject matte) throws Exception {
        super(context);
        url = source;
        matteIdentity = matte == null ? "" : matte.toString();
        setBackgroundColor(matte == null ? Color.BLACK : Color.TRANSPARENT);
        status = new TextView(context);
        status.setText("Connecting…");
        status.setTextColor(Color.parseColor(palette.getString("text")));
        status.setGravity(Gravity.CENTER);
        addView(status, new LayoutParams(-1, -1));
        mute = new Button(context);
        ShellChrome.control(mute,palette,false);
        ShellChrome.audioState(mute,title,muted,audioAvailability);
        mute.setOnClickListener(v -> {
            muted = !muted;
            if (matted != null) matted.setMuted(muted);
            if (player != null) player.setVolume(muted ? 0 : 1, muted ? 0 : 1);
            ShellChrome.audioState(mute,title,muted,audioAvailability);
        });
        int densityHeight=Math.round(ShellChrome.AUDIO_HEIGHT_DP*context.getResources().getDisplayMetrics().density);
        int densityWidth=Math.round(ShellChrome.AUDIO_WIDTH_DP*context.getResources().getDisplayMetrics().density);
        LayoutParams audioBounds=new LayoutParams(densityWidth,densityHeight,Gravity.TOP | Gravity.RIGHT);
        audioBounds.topMargin=audioBounds.rightMargin=Math.round(ShellChrome.AUDIO_INSET_DP*context.getResources().getDisplayMetrics().density);
        addView(mute,audioBounds);
        if (matte != null) {
            matted = new MattedVideoView(context, source, matte, new MattedVideoView.Listener() {
                public void prepared() { status.setVisibility(View.GONE); }
                public void audioTracks(int availability) {
                    audioAvailability=availability;
                    ShellChrome.audioState(mute,title,muted,audioAvailability);
                }
                public void failed(String message) {
                    audioTracks(DecoderAudio.UNKNOWN);
                    status.setText(message); status.setVisibility(View.VISIBLE);
                }
            });
            addView(matted, 0, new LayoutParams(-1, -1, Gravity.CENTER));
            return;
        }
        video = new VideoView(context);
        video.setZOrderMediaOverlay(true);
        addView(video, 0, new LayoutParams(-1, -1, Gravity.CENTER));
        video.setOnPreparedListener(p -> {
            player = p;
            audioAvailability=DecoderAudio.inspect(p);
            ShellChrome.audioState(mute,title,muted,audioAvailability);
            p.setVolume(muted ? 0 : 1, muted ? 0 : 1);
            status.setVisibility(View.GONE);
            if (active) video.start();
        });
        video.setOnErrorListener((p, what, extra) -> {
            player=null; audioAvailability=DecoderAudio.UNKNOWN;
            ShellChrome.audioState(mute,title,muted,audioAvailability);
            status.setText("Playback failed (" + what + ", " + extra + ")");
            status.setVisibility(View.VISIBLE);
            return true;
        });
        video.setOnCompletionListener(p -> {
            player=null; audioAvailability=DecoderAudio.UNKNOWN;
            ShellChrome.audioState(mute,title,muted,audioAvailability);
            status.setText("Stream ended"); status.setVisibility(View.VISIBLE);
        });
        video.setOnInfoListener((p,what,extra) -> {
            if(p==player && (what==MediaPlayer.MEDIA_INFO_METADATA_UPDATE || what==MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START)) {
                audioAvailability=DecoderAudio.inspect(p);
                ShellChrome.audioState(mute,title,muted,audioAvailability);
            }
            return false;
        });
        video.setVideoURI(Uri.parse(url));
    }

    boolean matches(String source, JSONObject matte) { return url.equals(source) && matteIdentity.equals(matte == null ? "" : matte.toString()); }
    Button muteControl() { return mute; }
    void dispose() { active = false; player = null; if (video != null) video.stopPlayback(); if (matted != null) matted.dispose(); }
}
