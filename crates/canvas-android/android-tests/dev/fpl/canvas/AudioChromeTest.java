package dev.fpl.canvas;

/** JVM-only contract checks: no device volume, decoder or Android runtime needed. */
public final class AudioChromeTest {
    public static void main(String[] args) {
        if (!ShellChrome.audioAction(true).equals("Unmute")) throw new AssertionError("muted action");
        if (!ShellChrome.audioAction(false).equals("Mute")) throw new AssertionError("unmuted action");
        if (!ShellChrome.audioDescription("KOMO",true).equals("Unmute KOMO audio. This stream is muted."))
            throw new AssertionError("source-specific muted state");
        if (!ShellChrome.audioDescription("Tour",false).equals("Mute Tour audio. This stream is unmuted."))
            throw new AssertionError("source-specific unmuted state");
        if (ShellChrome.audioAction(true).length()>6) throw new AssertionError("compact label");
        if (ShellChrome.AUDIO_HEIGHT_DP!=24 || ShellChrome.AUDIO_WIDTH_DP!=ShellChrome.AUDIO_HEIGHT_DP)
            throw new AssertionError("compact square D-pad target");
        if(DecoderAudio.fromKinds(null)!=DecoderAudio.UNKNOWN || DecoderAudio.fromKinds(new int[0])!=DecoderAudio.UNKNOWN)
            throw new AssertionError("missing track metadata is unknown");
        if(DecoderAudio.fromKinds(new int[]{1})!=DecoderAudio.ABSENT)
            throw new AssertionError("video-only metadata has no audio");
        if(DecoderAudio.fromKinds(new int[]{1,2})!=DecoderAudio.PRESENT || DecoderAudio.fromKinds(new int[]{2})!=DecoderAudio.PRESENT)
            throw new AssertionError("audio metadata is present, not necessarily audible");
        if(DecoderAudio.fromKinds(new int[]{0,4})!=DecoderAudio.UNKNOWN)
            throw new AssertionError("unrecognized and subtitle-only metadata are unknown");
        if(DecoderAudio.fromKinds(new int[]{1,0})!=DecoderAudio.UNKNOWN)
            throw new AssertionError("an unknown track could contain audio");
        if(DecoderAudio.fromKinds(new int[]{1,3,4,5})!=DecoderAudio.ABSENT)
            throw new AssertionError("video and known text/metadata tracks contain no audio");
        if(!ShellChrome.audioDescription("Deck",false,DecoderAudio.ABSENT).equals("No audio track in Deck.")
            || !ShellChrome.audioDescription("Deck",true,DecoderAudio.ABSENT).equals("No audio track in Deck."))
            throw new AssertionError("no-audio is independent from mute preference");
        if(!ShellChrome.audioDescription("Deck",false,DecoderAudio.UNKNOWN).contains("unknown"))
            throw new AssertionError("unknown audio must not imply presence");
        System.out.println("Audio chrome contracts passed");
    }
}
