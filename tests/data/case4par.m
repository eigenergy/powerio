function mpc = case4par
% Four bus case for element map tests (eigenergy/powerio#915, #916):
% sparse bus numbers, parallel lines 10-20 plus a reversed parallel 20-10,
% two generators on bus 10, an out of service generator, a tap transformer
% 30-20, a phase shifter 10-30, and an out of service branch 40-10.
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
	10	3	0	0	0	0	1	1.0	0	230	1	1.1	0.9;
	20	2	50	10	0	0	1	1.0	0	230	1	1.1	0.9;
	30	1	90	30	0	0	1	1.0	0	115	1	1.1	0.9;
	40	1	60	20	0	5	1	1.0	0	115	1	1.1	0.9;
];
mpc.gen = [
	10	50	0	300	-300	1.04	100	1	250	10;
	20	80	0	300	-300	1.02	100	1	300	10;
	10	30	0	300	-300	1.04	100	1	270	10;
	30	0	0	50	-50	1.0	100	0	100	0;
];
mpc.branch = [
	10	20	0.01	0.085	0.176	250	250	250	0	0	1	-360	360;
	10	20	0.01	0.085	0.176	250	250	250	0	0	1	-360	360;
	30	20	0.0	0.0586	0	0	0	0	0.98	0	1	-360	360;
	30	40	0.017	0.092	0.158	150	150	150	0	0	1	-360	360;
	40	10	0.032	0.161	0.306	150	150	150	0	0	0	-360	360;
	10	30	0.0	0.05	0	200	200	200	1.0	5	1	-360	360;
	20	10	0.012	0.09	0.18	250	250	250	0	0	1	-360	360;
];
mpc.gencost = [
	2	0	0	3	0.01	20	0;
	2	0	0	3	0.02	25	0;
	2	0	0	3	0.015	22	0;
	2	0	0	3	0.0	30	0;
];
