// A small Go client over the powerio C ABI: parses a case, extracts spans,
// and retains and releases every v6 handle type, so the ownership rules hold
// from a garbage collected runtime the way they do from C.
//
// Build flags come from the environment (see the CI job):
//
//	CGO_CFLAGS="-I .../powerio-capi/include" \
//	CGO_LDFLAGS="-L .../target/release -lpowerio_capi" go run . case9.m
package main

/*
#include <stdlib.h>
#include "powerio.h"
*/
import "C"

import (
	"fmt"
	"os"
	"unsafe"
)

func fail(what string, err *C.PioError) {
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %s: %s\n", what,
			C.GoString(C.pio_error_code(err)), C.GoString(C.pio_error_message(err)))
		C.pio_error_release(err)
	} else {
		fmt.Fprintln(os.Stderr, what)
	}
	os.Exit(1)
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: go-client <case.m>")
		os.Exit(2)
	}
	if C.pio_abi_version() != C.PIO_ABI_VERSION {
		fail("ABI version mismatch", nil)
	}

	path := C.CString(os.Args[1])
	defer C.free(unsafe.Pointer(path))

	// Parse into a module handle; a failure is a structured error handle.
	var cerr *C.PioError
	module := C.pio_parse_file(path, nil, &cerr)
	if module == nil {
		fail("parse", cerr)
	}
	if C.GoString(C.pio_module_kind(module)) != "balanced_network" {
		fail("kind", nil)
	}

	// The stored document round trips.
	doc := C.pio_module_write_json(module, &cerr)
	if doc == nil {
		fail("write", cerr)
	}
	reread := C.pio_module_read_json(doc, &cerr)
	C.pio_string_release(doc)
	if reread == nil {
		fail("reread", cerr)
	}

	// A structured refusal: a static value has no state to select.
	exported := C.pio_module_export_state(reread, 0, nil, &cerr)
	if exported != nil || cerr == nil {
		fail("static export should refuse", nil)
	}
	if C.GoString(C.pio_error_code(cerr)) != "REQUEST.STATE.NOT_A_COLLECTION" {
		fail("refusal code", cerr)
	}
	C.pio_error_release(cerr)
	cerr = nil

	// An unrecognized formula name carries its own registered code.
	bogus := C.CString("nodal_admittance")
	defer C.free(unsafe.Pointer(bogus))
	refused := C.pio_dc_data_build(reread, bogus, &cerr)
	if refused != nil || cerr == nil {
		fail("unknown formula should refuse", nil)
	}
	if C.GoString(C.pio_error_code(cerr)) != "REQUEST.CAPI.UNKNOWN_FORMULA" {
		fail("unknown formula code", cerr)
	}
	C.pio_error_release(cerr)
	cerr = nil

	// The PioDcData arrays outlive the module that built them.
	formula := C.CString("series_susceptance")
	defer C.free(unsafe.Pointer(formula))
	dc := C.pio_dc_data_build(reread, formula, &cerr)
	if dc == nil {
		fail("PioDcData build", cerr)
	}
	C.pio_module_release(reread)
	C.pio_module_release(module)

	rows := int(C.pio_dc_data_n_rows(dc))
	buses := int(C.pio_dc_data_n_buses(dc))
	if rows == 0 || buses == 0 {
		fail("PioDcData arrays are empty", nil)
	}
	if C.pio_dc_data_shift(dc) == nil {
		fail("shift span is nil", nil)
	}
	b := unsafe.Slice(C.pio_dc_data_susceptance(dc), rows)
	from := unsafe.Slice(C.pio_dc_data_from_indices(dc), rows)
	to := unsafe.Slice(C.pio_dc_data_to_indices(dc), rows)
	ids := unsafe.Slice(C.pio_dc_data_row_ids(dc), rows)
	for e := 0; e < rows; e++ {
		if from[e] < 0 || int(from[e]) >= buses || to[e] < 0 || int(to[e]) >= buses {
			fail("incidence index out of range", nil)
		}
		if b[e] == 0 {
			fail("zero susceptance row survived", nil)
		}
		if C.GoString(ids[e]) == "" {
			fail("row mapping is empty", nil)
		}
	}

	// Retain and release orders: a retained handle survives the original.
	kept := C.pio_dc_data_retain(dc)
	C.pio_dc_data_release(dc)
	if int(C.pio_dc_data_n_rows(kept)) != rows {
		fail("retained PioDcData lost rows", nil)
	}
	C.pio_dc_data_release(kept)
	C.pio_dc_data_release(nil)
	C.pio_module_release(nil)
	C.pio_error_release(nil)

	// The typed network handle carries the same lifecycle, independently
	// owned once taken from its module.
	again := C.pio_parse_file(path, nil, &cerr)
	if again == nil {
		fail("reparse", cerr)
	}
	net := C.pio_module_balanced_network(again, &cerr)
	if net == nil {
		fail("network", cerr)
	}
	C.pio_module_release(again)
	keptNet := C.pio_balanced_network_retain(net)
	C.pio_balanced_network_release(net)
	if C.pio_balanced_network_n_buses(keptNet) == 0 {
		fail("retained network lost its buses", nil)
	}
	C.pio_balanced_network_release(keptNet)
	C.pio_balanced_network_release(nil)

	fmt.Println("go client OK")
}
