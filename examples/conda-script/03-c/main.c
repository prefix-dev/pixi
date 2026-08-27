// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "gcc -o ${CACHE}/main ${SCRIPT} $(pkg-config --cflags --libs glib-2.0) && ${CACHE}/main"
//
// [dependencies]
// gcc = "*"
// glib = "*"
// pkg-config = "*"
// /// end-conda-script
#include <glib.h>

int main(void) {
    gchar *digest = g_compute_checksum_for_string(G_CHECKSUM_SHA256, "conda-script", -1);
    g_print("sha256(\"conda-script\") = %s\n", digest);
    g_free(digest);
    return 0;
}
