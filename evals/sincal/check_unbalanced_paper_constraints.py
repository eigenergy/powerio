#!/usr/bin/env python3
"""Reconstruct what the published unbalanced output numbers actually determine.
This is an output-data audit, NOT a reproduction of either paper's power flow,
a parser test, or evidence of native SINCAL execution. No missing input is fitted.
"""
import argparse
import cmath
import json
import math
from pathlib import Path


def vuf(magnitudes, angles):
    a = cmath.exp(2j*math.pi/3)
    v = [cmath.rect(m, math.radians(t)) for m, t in zip(magnitudes, angles, strict=True)]
    return 100*abs((v[0]+a*a*v[1]+a*v[2])/(v[0]+a*v[1]+a*a*v[2]))


def audit():
    # Arif et al., CC BY, Table 5, printed p.246. Angles are pair differences.
    # Set phase 1 to zero and take phi12/phi31; phi23 independently checks closure.
    rows = [
        ('load', [235.406]*3, [120, 120, 120]),
        ('load_pv', [236.266, 234.518, 236.806], [120.026, 118.529, 121.444]),
        ('load_pv_storage', [236.550, 234.212, 237.240], [120.039, 118.028, 121.931]),
    ]
    arif = [{'scenario': label, 'angle_closure_error_degrees': sum(phi)-360,
             'derived_vuf_percent_not_reported_in_paper': vuf(mags, [0, -phi[0], phi[2]])}
            for label, mags, phi in rows]
    # Vinayagam et al., Fig.12 magnitudes; Table V gives VUF about 1.48%.
    # These are two explicitly hypothetical interpretations, not fitted inputs.
    mags = [399, 406, 413]
    beta = sum(m**4 for m in mags)/sum(m*m for m in mags)**2
    ll_vuf = 100*math.sqrt((1-math.sqrt(3-6*beta))/(1+math.sqrt(3-6*beta)))
    return {
        'status': 'partial output-data reconstruction; neither load flow reproduced',
        'native_model_parsed': False,
        'arif2013': {'doi': '10.4236/sgre.2013.42029',
                     'url': 'https://file.scirp.org/Html/14-6401233_32197.htm',
                     'reference': 'Figure 4 and Table 5; printed pp.239,246',
                     'results': arif,
                     'missing_inputs': 'Line/neutral impedances and a complete transformer equivalent circuit are not specified. Rounded output phasors cannot validate a reader without matching network inputs.'},
        'vinayagam2015': {'doi': '10.2991/seee-15.2015.20',
                         'url': 'https://www.atlantis-press.com/article/25841476.pdf',
                         'reference': 'Figure 12 and Table V',
                         'reported_vuf_percent_approx': 1.48,
                         'assuming_phase_magnitudes_and_exact_120_degree_spacing_percent': vuf(mags, [0, -120, 120]),
                         'assuming_closed_line_line_magnitudes_percent': ll_vuf,
                         'conclusion': 'Neither simple interpretation reproduces 1.48%. Phase angles and the precise voltage basis/state are needed; this is not evidence that the paper is incorrect.'}}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('report', type=Path)
    args = p.parse_args()
    report = audit()
    args.report.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
