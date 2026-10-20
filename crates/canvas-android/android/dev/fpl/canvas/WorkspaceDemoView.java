package dev.fpl.canvas;

import android.content.Context;
import android.content.SharedPreferences;
import android.opengl.GLSurfaceView;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;
import java.util.ArrayDeque;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;
import org.json.JSONArray;
import org.json.JSONObject;

/** EGL/input/storage adapter only. Rust owns the shared workspace and mock actions. */
final class WorkspaceDemoView extends GLSurfaceView implements GLSurfaceView.Renderer {
    private static native long nativeCreate(String checkpoint);
    private static native String nativeFrame(long generation, int width, int height, double time, long now, String events);
    private static native String nativeCheckpoint(long generation);
    private static native void nativeDestroy(long generation);
    private final SharedPreferences storage;
    private final ArrayDeque<JSONObject> pending = new ArrayDeque<>();
    private final Handler ui = new Handler(Looper.getMainLooper());
    private long generation, lastSave;
    private int width, height;
    private volatile boolean failed, attached;
    private String checkpoint = "", description = "";
    private final Runnable tick = new Runnable() { public void run() {
        if (!attached || failed) return;
        requestRender(); ui.postDelayed(this, 100);
    }};
    WorkspaceDemoView(Context context) {
        super(context);
        storage = context.getSharedPreferences("canvas-workspace-demo", Context.MODE_PRIVATE);
        setEGLContextClientVersion(3);
        setEGLConfigChooser(8,8,8,8,0,0);
        setPreserveEGLContextOnPause(true);
        setRenderer(this); setRenderMode(RENDERMODE_WHEN_DIRTY);
        setFocusable(true); setFocusableInTouchMode(true);
        setContentDescription("Canvas mock workspace loading; native egui GLES");
    }
    @Override protected void onAttachedToWindow() { super.onAttachedToWindow(); attached=true; ui.post(tick); }
    @Override protected void onDetachedFromWindow() { attached=false; ui.removeCallbacks(tick); checkpointBeforePause(true); super.onDetachedFromWindow(); }
    @Override public void onPause() { ui.removeCallbacks(tick); checkpointBeforePause(false); super.onPause(); }
    @Override public void onResume() { super.onResume(); if(attached) { ui.removeCallbacks(tick); ui.post(tick); } }
    private void checkpointBeforePause(boolean destroy) {
        CountDownLatch complete = new CountDownLatch(1);
        queueEvent(() -> {
            try { if(generation!=0 && !failed) { save(); if(destroy) { nativeDestroy(generation); generation=0; } } }
            catch(Exception error) { showError(error); }
            finally { complete.countDown(); }
        });
        try { if(!complete.await(1,TimeUnit.SECONDS)) android.util.Log.e("CanvasWorkspace","Checkpoint timed out; retaining previous checkpoint"); }
        catch(InterruptedException error) { Thread.currentThread().interrupt(); }
    }
    @Override public void onSurfaceCreated(GL10 unused, EGLConfig config) {
        try { checkpoint=storage.getString("checkpoint", ""); generation=nativeCreate(checkpoint); lastSave=0; }
        catch(Exception error) { showError(error); }
    }
    @Override public void onSurfaceChanged(GL10 unused,int w,int h) { width=w; height=h; }
    @Override public void onDrawFrame(GL10 unused) {
        if(failed || generation==0 || width<=0 || height<=0) return;
        try {
            JSONArray events=new JSONArray();
            synchronized(pending) { while(!pending.isEmpty()) events.put(pending.removeFirst()); }
            String observed=nativeFrame(generation,width,height,SystemClock.elapsedRealtime()/1000.0,System.currentTimeMillis()/1000,events.toString());
            if(!observed.equals(description)) { description=observed; ui.post(() -> setContentDescription(observed)); }
            long now=SystemClock.elapsedRealtime();
            if(now-lastSave>=1000) { save(); lastSave=now; }
        } catch(Exception error) { showError(error); }
    }
    private void save() {
        String next=nativeCheckpoint(generation);
        if(!next.equals(checkpoint)) {
            if(!storage.edit().putString("checkpoint",next).commit()) throw new IllegalStateException("Could not persist local mock workspace");
            checkpoint=next;
        }
    }
    private void showError(Exception error) {
        failed=true;
        android.util.Log.e("CanvasWorkspace","Native workspace failed",error);
        ui.post(() -> {
            String message="Workspace preview failed: "+error.getMessage();
            setContentDescription(message); setVisibility(View.GONE);
            if(getParent() instanceof ViewGroup) {
                TextView report=new TextView(getContext()); report.setText(message); report.setTextColor(0xffff9daa); report.setBackgroundColor(0xff18191c); report.setTextSize(22);
                ((ViewGroup)getParent()).addView(report,getLayoutParams());
            }
        });
    }
    private boolean enqueue(JSONObject event) {
        synchronized(pending) { if(pending.size()>=64) { showError(new IllegalStateException("Workspace input queue full")); return true; } pending.add(event); }
        requestRender(); return true;
    }
    @Override public boolean onKeyDown(int code,KeyEvent event) {
        if(code==KeyEvent.KEYCODE_MENU) return false;
        try {
            String key=null;
            switch(code) {
                case KeyEvent.KEYCODE_DPAD_UP: key="Previous"; break;
                case KeyEvent.KEYCODE_DPAD_DOWN: key="Next"; break;
                case KeyEvent.KEYCODE_DPAD_LEFT: return enqueue(new JSONObject().put("key","Tab").put("shift",true));
                case KeyEvent.KEYCODE_DPAD_RIGHT: key="Tab"; break;
                case KeyEvent.KEYCODE_DPAD_CENTER: key="Activate"; break;
                case KeyEvent.KEYCODE_ENTER: key="Enter"; break;
                case KeyEvent.KEYCODE_BACK: case KeyEvent.KEYCODE_ESCAPE: key="Escape"; break;
                case KeyEvent.KEYCODE_MEDIA_FAST_FORWARD: key="E"; break;
                case KeyEvent.KEYCODE_MEDIA_REWIND: key="Z"; break;
                case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE: key="X"; break;
                case KeyEvent.KEYCODE_DEL: key="Backspace"; break;
                case KeyEvent.KEYCODE_TAB: key="Tab"; break;
                case KeyEvent.KEYCODE_CHANNEL_UP: case KeyEvent.KEYCODE_PAGE_UP: return enqueue(new JSONObject().put("scroll",300));
                case KeyEvent.KEYCODE_CHANNEL_DOWN: case KeyEvent.KEYCODE_PAGE_DOWN: return enqueue(new JSONObject().put("scroll",-300));
                default:
                    if(code>=KeyEvent.KEYCODE_A && code<=KeyEvent.KEYCODE_Z) key=String.valueOf((char)('A'+code-KeyEvent.KEYCODE_A));
                    else if(code>=KeyEvent.KEYCODE_0 && code<=KeyEvent.KEYCODE_9) key=String.valueOf((char)('0'+code-KeyEvent.KEYCODE_0));
            }
            if(key!=null) enqueue(new JSONObject().put("key",key).put("shift",event.isShiftPressed()).put("command",event.isCtrlPressed()||event.isMetaPressed()));
            int unicode=event.getUnicodeChar();
            if(unicode>=32 && !event.isCtrlPressed() && !event.isMetaPressed()) enqueue(new JSONObject().put("text",new String(Character.toChars(unicode))));
            return key!=null || unicode>=32 || super.onKeyDown(code,event);
        } catch(Exception error) { showError(error); return true; }
    }
    @Override public boolean onTouchEvent(MotionEvent event) {
        if(event.getActionMasked()!=MotionEvent.ACTION_DOWN && event.getActionMasked()!=MotionEvent.ACTION_UP) return true;
        try { requestFocus(); return enqueue(new JSONObject().put("x",event.getX()).put("y",event.getY()).put("pressed",event.getActionMasked()==MotionEvent.ACTION_DOWN)); }
        catch(Exception error) { showError(error); return true; }
    }
}
