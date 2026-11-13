// A small Go client for the PowerIO C ABI.
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

func text(view C.PioStringView) string {
	if view.data == nil || view.len == 0 {
		return ""
	}
	return C.GoStringN(view.data, C.int(view.len))
}

func fail(operation string, err *C.PioError) {
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %s: %s\n", operation,
			text(C.pio_error_code(err)), text(C.pio_error_message(err)))
		C.pio_error_release(err)
	} else {
		fmt.Fprintln(os.Stderr, operation)
	}
	os.Exit(1)
}

// compatible reports whether the loaded library works with the header this
// client was compiled against: the header's release series (the same major
// and minor numbers before 1.0, the same major number from 1.0 on), at the
// header's release or a newer one.
func compatible(library C.PioVersionView) bool {
	if library.major != C.PIO_VERSION_MAJOR {
		return false
	}
	if library.minor != C.PIO_VERSION_MINOR {
		return C.PIO_VERSION_MAJOR > 0 && library.minor > C.PIO_VERSION_MINOR
	}
	return library.patch >= C.PIO_VERSION_PATCH
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: go-client case9.m")
		os.Exit(2)
	}
	if !compatible(C.pio_version_numbers()) {
		fail("the library is not in this header's release series, or it is older", nil)
	}

	path := C.CString(os.Args[1])
	defer C.free(unsafe.Pointer(path))
	var cerr *C.PioError
	source := C.pio_source_open(path, C.size_t(len(os.Args[1])), &cerr)
	if source == nil {
		fail("source", cerr)
	}
	module := C.pio_parse(source, nil, 0, &cerr)
	C.pio_source_release(source)
	if module == nil {
		fail("parse", cerr)
	}

	value := C.pio_module_value(module)
	if value == nil || text(C.pio_value_type_name(value)) != "powerio.BalancedNetwork" {
		fail("value type", nil)
	}
	network := C.pio_value_balanced_network(value, &cerr)
	C.pio_value_release(value)
	if network == nil {
		fail("balanced network", cerr)
	}
	C.pio_module_release(module)

	if C.pio_balanced_network_bus_count(network) != 9 {
		fail("unexpected bus count", nil)
	}
	incidence := C.pio_calc_incidence_matrix(network, nil, 0, &cerr)
	if incidence == nil {
		fail("incidence matrix", cerr)
	}
	if C.pio_sparse_matrix_rows(incidence) != C.pio_balanced_network_branch_count(network) {
		fail("incidence row count", nil)
	}
	C.pio_sparse_matrix_release(incidence)
	C.pio_balanced_network_release(network)

	fmt.Println("PowerIO Go probe passed")
}
