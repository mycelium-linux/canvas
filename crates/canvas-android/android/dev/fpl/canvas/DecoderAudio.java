package dev.fpl.canvas;

import android.media.MediaPlayer;

/** Decoder track metadata. A track is not evidence of audible samples. */
final class DecoderAudio {
    static final int UNKNOWN=0, PRESENT=1, ABSENT=2;
    static int fromKinds(int[] kinds) {
        if(kinds==null || kinds.length==0) return UNKNOWN;
        boolean video=false, unknown=false;
        for(int kind:kinds) {
            if(kind==MediaPlayer.TrackInfo.MEDIA_TRACK_TYPE_AUDIO) return PRESENT;
            if(kind==MediaPlayer.TrackInfo.MEDIA_TRACK_TYPE_VIDEO) video=true;
            else if(kind!=MediaPlayer.TrackInfo.MEDIA_TRACK_TYPE_TIMEDTEXT
                && kind!=MediaPlayer.TrackInfo.MEDIA_TRACK_TYPE_SUBTITLE
                && kind!=MediaPlayer.TrackInfo.MEDIA_TRACK_TYPE_METADATA) unknown=true;
        }
        return video && !unknown ? ABSENT : UNKNOWN;
    }
    static int inspect(MediaPlayer player) {
        if(player==null) return UNKNOWN;
        try {
            MediaPlayer.TrackInfo[] tracks=player.getTrackInfo();
            if(tracks==null) return UNKNOWN;
            int[] kinds=new int[tracks.length];
            for(int i=0;i<tracks.length;i++) kinds[i]=tracks[i]==null ? 0 : tracks[i].getTrackType();
            return fromKinds(kinds);
        } catch(RuntimeException error) {
            android.util.Log.w("CanvasAudio","Audio track metadata unavailable",error);
            return UNKNOWN;
        }
    }
    private DecoderAudio() {}
}
