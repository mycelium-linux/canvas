package dev.fpl.canvas;

import android.animation.ValueAnimator;
import android.view.View;
import android.view.animation.Interpolator;

/** Platform compositor adapter; easing samples belong to shared Rust GFX. */
final class TvMotion {
    private static native float[] curve();
    private static final float[] CURVE = curve();
    private static final Interpolator EASING = input -> {
        float position=Math.max(0,Math.min(1,input))*(CURVE.length-1);
        int index=Math.min((int)position,CURVE.length-2);
        float fraction=position-index;
        return CURVE[index]*(1-fraction)+CURVE[index+1]*fraction;
    };
    static void focus(View view,boolean focused) {
        view.animate().cancel();
        float scale=focused?1.025f:1f;
        if(android.os.Build.VERSION.SDK_INT>=26 && !ValueAnimator.areAnimatorsEnabled()) {view.setScaleX(scale);view.setScaleY(scale);return;}
        // Transform only: no measure/layout churn, continuous retargeting from current pose.
        view.animate().scaleX(scale).scaleY(scale).setDuration(120).setInterpolator(EASING).start();
    }
    static void reveal(View view) {
        view.animate().cancel();
        if(android.os.Build.VERSION.SDK_INT>=26 && !ValueAnimator.areAnimatorsEnabled()) {view.setAlpha(1);return;}
        view.setAlpha(0);
        view.animate().alpha(1).setDuration(160).setInterpolator(EASING).start();
    }
    static void pose(View view,float x,float rotation) {
        view.animate().cancel();
        if(android.os.Build.VERSION.SDK_INT>=26 && !ValueAnimator.areAnimatorsEnabled()) {view.setTranslationX(x);view.setRotation(rotation);return;}
        view.animate().translationX(x).rotation(rotation).setDuration(180).setInterpolator(EASING).start();
    }
    private TvMotion() {}
}
