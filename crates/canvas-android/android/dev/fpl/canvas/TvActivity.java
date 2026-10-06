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
import android.widget.ImageView;
import android.widget.ScrollView;
import android.graphics.Bitmap;
import android.graphics.Paint;
import android.view.SurfaceView;
import android.view.SurfaceHolder;
import android.os.HandlerThread;
import android.opengl.EGL14;
import android.opengl.EGLDisplay;
import android.opengl.EGLContext;
import android.opengl.EGLSurface;
import android.opengl.EGLConfig;
import android.opengl.GLES30;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Calendar;
import android.graphics.pdf.PdfRenderer;
import android.os.ParcelFileDescriptor;
import java.io.File;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.Future;
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
    private static native String nativePdfAsset(String asset);
    private static native float[] nativeClockFrame(int width, int height, int hour, int minute, float second);
    private static native String nativeBackgroundProgram(String id);
    private GpuBackground background;
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
    private final ThreadPoolExecutor pdfJobs = new ThreadPoolExecutor(1, 1, 30, TimeUnit.SECONDS,
        new ArrayBlockingQueue<Runnable>(8));

    /** Independent 720p EGL surface; no CPU readback or 4K UI texture copy. */
    private final class GpuBackground extends SurfaceView implements SurfaceHolder.Callback {
        private final HandlerThread worker = new HandlerThread("canvas-background-gpu");
        private final Handler gpu;
        private EGLDisplay display = EGL14.EGL_NO_DISPLAY;
        private EGLContext context = EGL14.EGL_NO_CONTEXT;
        private EGLSurface output = EGL14.EGL_NO_SURFACE;
        private int program, buffer, width, height;
        private volatile boolean active;
        private String key = "", vertex = "", fragment = "";
        private float speed;
        private final float[] theme = new float[4];
        private final ByteBuffer uniforms = ByteBuffer.allocateDirect(48).order(ByteOrder.nativeOrder());
        private final long epoch = System.nanoTime();
        private long completedFrames, workNanos, sampleStart = System.nanoTime();
        GpuBackground() {
            super(TvActivity.this);
            worker.start(); gpu = new Handler(worker.getLooper());
            setFocusable(false); getHolder().addCallback(this);
            setVisibility(View.GONE);
        }
        private void failure(Exception error) {
            active = false;
            android.util.Log.e("CanvasBackground", "GPU background failed", error);
            runOnUiThread(() -> { setVisibility(View.GONE); showStatus("GPU background: " + error.getMessage()); });
        }
        void configure(JSONObject document, JSONObject palette) throws Exception {
            JSONObject spec = document.optJSONObject("background");
            if (spec == null || !spec.optString("kind").equals("shader")) {
                active = false; setVisibility(View.GONE); gpu.removeCallbacks(draw); key = "";
                gpu.post(this::releaseProgram);
                return;
            }
            String next = spec.toString() + palette.getString("background");
            if (next.equals(key)) return;
            JSONObject source = new JSONObject(nativeBackgroundProgram(spec.getString("shader_id")));
            if (source.has("error")) throw new IllegalArgumentException(source.getString("error"));
            float rate = (float)spec.getDouble("speed");
            if (!Float.isFinite(rate) || rate < 0 || rate > 4) throw new IllegalArgumentException("Invalid shader speed");
            int color = Color.parseColor(palette.getString("background"));
            final String vs = source.getString("vertex"), fs = source.getString("fragment");
            key = next; setVisibility(View.VISIBLE); active = resumed;
            gpu.post(() -> {
                vertex = vs; fragment = fs; speed = rate;
                theme[0] = Color.red(color) / 255f; theme[1] = Color.green(color) / 255f;
                theme[2] = Color.blue(color) / 255f; theme[3] = 1;
                try { if (context != EGL14.EGL_NO_CONTEXT) installProgram(); schedule(); }
                catch (Exception error) { failure(error); }
            });
        }
        void setActive(boolean visible) {
            active = visible && getVisibility() == View.VISIBLE;
            gpu.removeCallbacks(draw);
            if (active) gpu.post(() -> {
                completedFrames = 0; workNanos = 0; sampleStart = System.nanoTime(); schedule();
            });
        }
        private int shader(int kind, String code) {
            int shader = GLES30.glCreateShader(kind);
            GLES30.glShaderSource(shader, code); GLES30.glCompileShader(shader);
            int[] ok = new int[1]; GLES30.glGetShaderiv(shader, GLES30.GL_COMPILE_STATUS, ok, 0);
            if (ok[0] == 0) {
                String reason = GLES30.glGetShaderInfoLog(shader); GLES30.glDeleteShader(shader);
                throw new IllegalStateException(reason);
            }
            return shader;
        }
        private void releaseProgram() {
            if (program != 0) GLES30.glDeleteProgram(program);
            if (buffer != 0) GLES30.glDeleteBuffers(1, new int[]{buffer}, 0);
            program = 0; buffer = 0;
        }
        private void installProgram() {
            releaseProgram();
            int vs = shader(GLES30.GL_VERTEX_SHADER, vertex), fs = 0;
            try {
                fs = shader(GLES30.GL_FRAGMENT_SHADER, fragment);
                program = GLES30.glCreateProgram(); GLES30.glAttachShader(program, vs); GLES30.glAttachShader(program, fs);
                GLES30.glLinkProgram(program);
                int[] ok = new int[1]; GLES30.glGetProgramiv(program, GLES30.GL_LINK_STATUS, ok, 0);
                if (ok[0] == 0) throw new IllegalStateException(GLES30.glGetProgramInfoLog(program));
                int[] buffers = new int[1]; GLES30.glGenBuffers(1, buffers, 0); buffer = buffers[0];
                GLES30.glBindBuffer(GLES30.GL_UNIFORM_BUFFER, buffer);
                GLES30.glBufferData(GLES30.GL_UNIFORM_BUFFER, 48, null, GLES30.GL_DYNAMIC_DRAW);
                GLES30.glBindBufferBase(GLES30.GL_UNIFORM_BUFFER, 0, buffer);
            } finally { GLES30.glDeleteShader(vs); if (fs != 0) GLES30.glDeleteShader(fs); }
        }
        private void schedule() { gpu.removeCallbacks(draw); if (active) gpu.post(draw); }
        private final Runnable draw = new Runnable() {
            @Override public void run() {
                if (!active || program == 0 || output == EGL14.EGL_NO_SURFACE) return;
                try {
                    long started = System.nanoTime();
                    uniforms.clear(); uniforms.putFloat(width).putFloat(height);
                    uniforms.putFloat(speed == 0 ? 0 : (System.nanoTime() - epoch) / 1e9f * speed).putFloat(0);
                    for (int i = 0; i < 4; i++) uniforms.putFloat(0);
                    for (float value : theme) uniforms.putFloat(value);
                    uniforms.flip();
                    GLES30.glViewport(0, 0, width, height); GLES30.glUseProgram(program);
                    GLES30.glBindBuffer(GLES30.GL_UNIFORM_BUFFER, buffer);
                    GLES30.glBufferSubData(GLES30.GL_UNIFORM_BUFFER, 0, 48, uniforms);
                    GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
                    int error = GLES30.glGetError();
                    if (error != GLES30.GL_NO_ERROR) throw new IllegalStateException("GL error " + error);
                    if (!EGL14.eglSwapBuffers(display, output)) throw new IllegalStateException("EGL swap failed");
                    // One submitted frame at a time; wait for completion before pacing.
                    GLES30.glFinish();
                    workNanos += System.nanoTime() - started;
                    if (++completedFrames == 300) {
                        long now = System.nanoTime();
                        android.util.Log.i("CanvasBackground", "fps=" + (completedFrames * 1e9 / (now - sampleStart))
                            + " mean-frame-work-ms=" + (workNanos / 1e6 / completedFrames));
                        completedFrames = 0; workNanos = 0; sampleStart = now;
                    }
                    if (active && speed > 0) gpu.postDelayed(this,
                        Math.max(1, 33 - TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)));
                } catch (Exception error) { failure(error); }
            }
        };
        @Override public void surfaceCreated(SurfaceHolder holder) {
            int w = Math.max(1, getWidth()), h = Math.max(1, getHeight());
            float scale = Math.min(1, Math.min(1280f / w, 720f / h));
            width = Math.max(1, Math.round(w * scale)); height = Math.max(1, Math.round(h * scale));
            holder.setFixedSize(width, height);
            gpu.post(() -> {
                try {
                    display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
                    if (!EGL14.eglInitialize(display, new int[2], 0, new int[2], 0)) throw new IllegalStateException("EGL initialize failed");
                    EGLConfig[] configs = new EGLConfig[1]; int[] count = new int[1];
                    int[] attributes = {EGL14.EGL_RENDERABLE_TYPE, 0x40, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
                        EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8, EGL14.EGL_NONE};
                    if (!EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, count, 0) || count[0] == 0)
                        throw new IllegalStateException("No GLES3 window config");
                    context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                        new int[]{EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
                    output = EGL14.eglCreateWindowSurface(display, configs[0], holder.getSurface(), new int[]{EGL14.EGL_NONE}, 0);
                    if (!EGL14.eglMakeCurrent(display, output, output, context)) throw new IllegalStateException("EGL context failed");
                    completedFrames = 0; workNanos = 0; sampleStart = System.nanoTime();
                    android.util.Log.i("CanvasBackground", "GPU=" + GLES30.glGetString(GLES30.GL_RENDERER)
                        + " GL=" + GLES30.glGetString(GLES30.GL_VERSION) + " buffer=" + width + "x" + height);
                    if (!vertex.isEmpty()) installProgram(); schedule();
                } catch (Exception error) { failure(error); }
            });
        }
        private void release() {
            gpu.removeCallbacks(draw); releaseProgram();
            if (display != EGL14.EGL_NO_DISPLAY) {
                EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
                if (output != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, output);
                if (context != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, context);
                EGL14.eglTerminate(display);
            }
            display = EGL14.EGL_NO_DISPLAY; context = EGL14.EGL_NO_CONTEXT; output = EGL14.EGL_NO_SURFACE;
        }
        @Override public void surfaceDestroyed(SurfaceHolder holder) {
            gpu.removeCallbacks(draw);
            java.util.concurrent.CountDownLatch released = new java.util.concurrent.CountDownLatch(1);
            if (gpu.post(() -> { release(); released.countDown(); })) {
                try {
                    if (!released.await(2, TimeUnit.SECONDS)) failure(new IllegalStateException("GPU teardown timed out"));
                } catch (InterruptedException error) { Thread.currentThread().interrupt(); failure(error); }
            }
        }
        @Override public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {
            gpu.post(() -> { width = w; height = h; schedule(); });
        }
        void dispose() { active = false; gpu.post(() -> { release(); worker.quitSafely(); }); }
    }

    /** Paints canonical gfx primitives; no duplicate clock geometry in Java. */
    private final class AnalogClockView extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Calendar time = Calendar.getInstance();
        private final int[] inks;
        AnalogClockView(JSONObject palette) throws Exception {
            super(TvActivity.this);
            inks = new int[]{Color.parseColor(palette.getString("surface")),
                Color.parseColor(palette.getString("text_muted")),
                Color.parseColor(palette.getString("text")),
                Color.parseColor(palette.getString("primary"))};
            setBackgroundColor(Color.TRANSPARENT);
            setContentDescription("Analog clock");
            paint.setStrokeCap(Paint.Cap.ROUND);
        }
        @Override protected void onDraw(android.graphics.Canvas target) {
            super.onDraw(target);
            if (!resumed || getWidth() == 0 || getHeight() == 0) return;
            time.setTimeInMillis(System.currentTimeMillis());
            float density = getResources().getDisplayMetrics().density;
            target.save(); target.scale(density, density);
            float[] records = nativeClockFrame(Math.max(1, Math.round(getWidth() / density)), Math.max(1, Math.round(getHeight() / density)), time.get(Calendar.HOUR_OF_DAY),
                time.get(Calendar.MINUTE), time.get(Calendar.SECOND) + time.get(Calendar.MILLISECOND) / 1000f);
            if (records == null || records.length != 65 * 8) throw new IllegalStateException("Invalid clock primitives");
            for (int i = 0; i < records.length; i += 8) {
                paint.setColor(inks[(int)records[i + 6]]);
                if (records[i] == 1f) {
                    paint.setStyle(Paint.Style.FILL);
                    target.drawCircle(records[i + 1], records[i + 2], records[i + 3], paint);
                    if (records[i + 5] > 0) {
                        paint.setStyle(Paint.Style.STROKE);
                        paint.setStrokeWidth(records[i + 5]);
                        paint.setColor(inks[(int)records[i + 7]]);
                        target.drawCircle(records[i + 1], records[i + 2], records[i + 3], paint);
                    }
                } else if (records[i] == 5f) {
                    paint.setStyle(Paint.Style.STROKE); paint.setStrokeWidth(records[i + 5]);
                    target.drawLine(records[i + 1], records[i + 2], records[i + 3], records[i + 4], paint);
                } else throw new IllegalStateException("Unsupported clock primitive");
            }
            target.restore();
            postInvalidateDelayed(40);
        }
    }

    /** Platform raster adapter only; source identity and geometry come from Rust. */
    private final class PdfSurface extends LinearLayout {
        private final ImageView image;
        private final ScrollView scroll;
        private final LinearLayout controls;
        private final TextView label;
        private final Button previous, next, sizing;
        private final String asset;
        private final String title;
        private final int rasterWidth, viewWidth, tileHeight;
        private int viewHeight;
        private boolean fitPage;
        private int pageIndex, pageCount, generation;
        private boolean disposed, busy;
        private Bitmap bitmap;
        private Future<?> pending;

        PdfSurface(JSONObject source, String title, JSONObject palette, int width, int height) throws Exception {
            super(TvActivity.this);
            this.asset = source.toString(); this.title = title;
            viewWidth = width;
            tileHeight = height;
            viewHeight = Math.max(1, height - dp(28));
            fitPage = width < viewHeight;
            rasterWidth = Math.max(1, Math.min(width, 1920));
            setOrientation(VERTICAL);
            // The page is opaque paper; unused fit/scroll bounds are not chrome.
            setBackgroundColor(Color.TRANSPARENT);
            image = new ImageView(TvActivity.this);
            image.setScaleType(ImageView.ScaleType.FIT_CENTER);
            image.setAdjustViewBounds(true);
            scroll = new ScrollView(TvActivity.this);
            scroll.setFillViewport(false);
            scroll.setFocusable(true);
            scroll.addView(image, new ScrollView.LayoutParams(-1, -2));
            addView(scroll, new LinearLayout.LayoutParams(-1, 0, 1));
            controls = new LinearLayout(TvActivity.this);
            controls.setGravity(Gravity.CENTER_VERTICAL);
            previous = new Button(TvActivity.this); previous.setText("‹");
            next = new Button(TvActivity.this); next.setText("›");
            sizing = new Button(TvActivity.this); sizing.setTextSize(12);
            sizing.setText(fitPage ? "Read" : "Fit");
            styleControl(previous, palette, false); styleControl(next, palette, false);
            styleControl(sizing, palette, false);
            label = new TextView(TvActivity.this); label.setTextSize(12);
            label.setGravity(Gravity.CENTER);
            label.setTextColor(Color.parseColor(palette.getString("text")));
            controls.addView(previous, new LinearLayout.LayoutParams(80, -1));
            controls.addView(label, new LinearLayout.LayoutParams(0, -1, 1));
            controls.addView(sizing, new LinearLayout.LayoutParams(dp(72), -1));
            controls.addView(next, new LinearLayout.LayoutParams(80, -1));
            addView(controls, new LinearLayout.LayoutParams(-1, dp(28)));
            previous.setOnClickListener(v -> render(pageIndex - 1));
            next.setOnClickListener(v -> render(pageIndex + 1));
            sizing.setOnClickListener(v -> toggleSizing());
            scroll.setOnKeyListener((v, key, event) -> {
                if (key != KeyEvent.KEYCODE_DPAD_CENTER && key != KeyEvent.KEYCODE_ENTER) return false;
                if (event.getAction() == KeyEvent.ACTION_UP) toggleSizing();
                return true;
            });
            render(0);
        }

        private void toggleSizing() {
            fitPage = !fitPage;
            sizing.setText(fitPage ? "Read" : "Fit");
            if (bitmap != null) applyPageBounds(bitmap);
        }

        private void applyPageBounds(Bitmap page) {
            image.setMaxWidth(viewWidth);
            image.setMaxHeight(fitPage ? viewHeight : Integer.MAX_VALUE);
            image.setLayoutParams(new ScrollView.LayoutParams(-1, fitPage ? viewHeight
                : Math.max(1, (int)((float)page.getHeight() / page.getWidth() * viewWidth))));
            scroll.scrollTo(0, 0);
        }

        private void render(int requested) {
            if (disposed || busy || requested < 0 || (pageCount > 0 && requested >= pageCount)) return;
            busy = true;
            final int ticket = ++generation;
            label.setText(title + " · Loading…");
            try {
                pending = pdfJobs.submit(() -> {
                    Bitmap result = null;
                    try {
                        JSONObject verified = new JSONObject(nativePdfAsset(asset));
                        if (verified.has("error")) throw new IllegalArgumentException(verified.getString("error"));
                        try (ParcelFileDescriptor file = ParcelFileDescriptor.open(new File(verified.getString("path")), ParcelFileDescriptor.MODE_READ_ONLY);
                             PdfRenderer renderer = new PdfRenderer(file)) {
                            final int count = renderer.getPageCount();
                            if (requested >= count) throw new IllegalArgumentException("Page out of range");
                            try (PdfRenderer.Page page = renderer.openPage(requested)) {
                                float scale = Math.min((float)rasterWidth / page.getWidth(), 2560f / page.getHeight());
                                result = Bitmap.createBitmap(Math.max(1, (int)(page.getWidth() * scale)),
                                    Math.max(1, (int)(page.getHeight() * scale)), Bitmap.Config.ARGB_8888);
                                result.eraseColor(Color.WHITE);
                                page.render(result, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY);
                            }
                            final Bitmap ready = result;
                            runOnUiThread(() -> {
                                if (disposed || generation != ticket) { ready.recycle(); return; }
                                controls.setVisibility(count == 1 ? View.GONE : View.VISIBLE);
                                viewHeight = Math.max(1, tileHeight - (count == 1 ? 0 : dp(28)));
                                applyPageBounds(ready);
                                image.setImageBitmap(ready);
                                if (bitmap != null) bitmap.recycle();
                                bitmap = ready; pageIndex = requested; pageCount = count; busy = false;
                                label.setText(title + " · " + (requested + 1) + " / " + count);
                                previous.setEnabled(requested > 0); next.setEnabled(requested + 1 < count);
                            });
                        }
                    } catch (Exception error) {
                        if (result != null) result.recycle();
                        runOnUiThread(() -> {
                            if (!disposed && generation == ticket) {
                                busy = false; label.setText(title + " · PDF error: " + error.getMessage());
                            }
                        });
                    }
                });
            } catch (java.util.concurrent.RejectedExecutionException error) {
                busy = false; label.setText(title + " · PDF renderer busy");
            }
        }

        void dispose() {
            disposed = true; generation++;
            if (pending != null) pending.cancel(false);
            pdfJobs.purge();
            image.setImageDrawable(null);
            if (bitmap != null) { bitmap.recycle(); bitmap = null; }
        }
    }

    @Override public void onDestroy() {
        if (background != null) background.dispose();
        for (View widget : widgets) if (widget instanceof PdfSurface) ((PdfSurface)widget).dispose();
        pdfJobs.shutdownNow();
        super.onDestroy();
    }
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
        background = new GpuBackground();
        frame.addView(background, new FrameLayout.LayoutParams(-1, -1));
        video = new VideoView(this);
        video.setZOrderMediaOverlay(true); // Above shader surface, below native UI.
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
        background.setActive(true);
        lastProjection = "";
        if (workspaceMode) currentUrl = "";
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        video.start();
        updates.post(refresh);
    }

    @Override protected void onPause() {
        resumed = false;
        background.setActive(false);
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
            background.configure(projection.getJSONObject("document"), palette);
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
                button.setTextSize(12);
                styleControl(button, palette, projectId.equals(catalog.getString("selected_project")));
                button.setOnClickListener(v -> {
                    String error = nativeSelectWorkspace(projectId);
                    if (error == null || !error.isEmpty()) showStatus("Switch failed: " + error);
                    else { lastProjection = ""; refreshWorkspace(); }
                });
                button.setMinHeight(0); button.setMinimumHeight(0);
                LinearLayout.LayoutParams item = new LinearLayout.LayoutParams(-2, barHeight() - dp(4));
                item.setMargins(dp(4), dp(2), 0, dp(2));
                selector.addView(button, item);
                if (projectId.equals(focusedProject)) button.requestFocus();
            }
            for (View widget : widgets) {
                if (widget instanceof PdfSurface) ((PdfSurface)widget).dispose();
                frame.removeView(widget);
            }
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
                } else if (content.getString("kind").equals("asset")
                    && content.getJSONObject("asset").getString("media_type").equals("application/pdf")) {
                    PdfSurface pdf = new PdfSurface(content.getJSONObject("asset"), surface.getString("title"), palette, w, h);
                    frame.addView(pdf, geometry); widgets.add(pdf);
                } else if (content.getString("kind").equals("widget")
                    && content.getJSONObject("spec").getString("kind").equals("clock")
                    && content.getJSONObject("spec").optString("mode", "digital").equals("analog")) {
                    AnalogClockView clock = new AnalogClockView(palette);
                    frame.addView(clock, geometry); widgets.add(clock);
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
        return dp(32);
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }
}
