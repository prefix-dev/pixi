// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "go run ${SCRIPT}"
//
// [dependencies]
// go = "*"
// gcc = "*"
// pkg-config = "*"
// zlib = "*"
// /// end-conda-script
package main

/*
#cgo pkg-config: zlib
#include <zlib.h>
*/
import "C"

import "fmt"

func main() {
	message := []byte("conda-script")
	checksum := C.adler32(1, (*C.Bytef)(&message[0]), C.uInt(len(message)))
	fmt.Printf("adler32(\"conda-script\") = 0x%08x\n", uint64(checksum))
}
