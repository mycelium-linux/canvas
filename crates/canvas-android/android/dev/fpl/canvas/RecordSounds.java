package dev.fpl.canvas;

import android.content.Context;
import android.media.AudioAttributes;
import android.media.SoundPool;
import java.io.File;
import java.io.FileOutputStream;

/** Optional platform sonification; no audio focus request or studio routing changes. */
final class RecordSounds {
    private static native byte[] selectionWav();
    private final SoundPool pool=new SoundPool.Builder().setMaxStreams(1).setAudioAttributes(new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build()).build();
    private volatile int sample;
    private volatile boolean disposed;
    private boolean enabled;
    RecordSounds(Context host,boolean enabled){this.enabled=enabled;
        pool.setOnLoadCompleteListener((owner,id,status)->{if(status==0&&!disposed)sample=id;else if(status!=0)android.util.Log.w("CanvasRecords","Selection cue unavailable: "+status);});
        new Thread(()->{try {
            byte[] wav=selectionWav();if(wav==null)throw new IllegalStateException("Selection cue unavailable");
            File file=new File(host.getCacheDir(),"record-selection-v1.wav");try(FileOutputStream output=new FileOutputStream(file)){output.write(wav);}
            if(!disposed)pool.load(file.getAbsolutePath(),1);
        }catch(Exception error){android.util.Log.w("CanvasRecords","Selection cue unavailable",error);}},"canvas-record-cue").start();
    }
    void enabled(boolean value){enabled=value;}
    void play(){if(enabled&&!disposed&&sample!=0)pool.play(sample,.25f,.25f,0,0,1f);}
    void dispose(){disposed=true;pool.release();}
}
