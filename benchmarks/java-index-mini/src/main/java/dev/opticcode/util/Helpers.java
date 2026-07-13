package dev.opticcode.util;

public final class Helpers {
    public static final String READY = "ready";

    private Helpers() {
    }

    public static String create() {
        return READY;
    }

    public static String create(String value) {
        return value;
    }

    public static void ping() {
    }
}
