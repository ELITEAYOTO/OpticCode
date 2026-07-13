package dev.opticcode.app;

import dev.opticcode.model.Feature;
import dev.opticcode.model.Material;
import dev.opticcode.wild.*;

import static dev.opticcode.util.Helpers.create;
import static dev.opticcode.util.Helpers.*;

@Feature
public final class Plugin {
    // Material.GUNPOWDER in documentation is not a reference.
    private Material material = Material.GUNPOWDER;
    private WildService service = new WildService();
    private Peer peer = new Peer();
    private String text = "Material.GUNPOWDER";
    private MissingType missing;

    public void start() {
        create(text);
        ping();
        dev.opticcode.alpha.Duplicate duplicate = new dev.opticcode.alpha.Duplicate();
        service.run();
    }

    public final class Inner {
        public void run() {
        }

        public void run(String value) {
        }
    }
}
