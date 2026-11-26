function answer = solve_case(path)
  options = mpoption('verbose', 0, 'out.all', 0, 'pf.alg', 'NR', ...
                     'pf.tol', 1e-10, 'pf.enforce_q_lims', 0);
  [solution, ok] = runpf(path, options);
  if !ok, error('MATPOWER did not converge'); endif
  voltage = struct();
  for k = 1:rows(solution.bus)
    b = solution.bus(k, :);
    if b(2) != 4
      voltage.(num2str(b(1))) = [b(8), b(9)];
    endif
  endfor
  branches = {};
  for k = 1:rows(solution.branch)
    b = solution.branch(k, :);
    if b(11) == 1
      branches{end+1} = {num2str(b(1)), num2str(b(2)), b(14), b(15), b(16), b(17)};
    endif
  endfor
  internal = ext2int(solution);
  [Y, ~, ~] = makeYbus(internal);
  [row, col, y] = find(Y);
  answer = struct('status', 'solved', 'solver', ['MATPOWER ', mpver(), ' on GNU Octave ', version()], ...
                  'units', 'pu', 'voltage', voltage);
  answer.branch_power = branches;
  answer.admittance = struct('basis', 'network-pu', 'base_mva', solution.baseMVA);
  answer.admittance.labels = arrayfun(@num2str, internal.order.bus.i2e, 'UniformOutput', false);
  answer.admittance.entries = [row - 1, col - 1, real(y), imag(y)];
endfunction

args = argv();
addpath(args{1});
install_matpower(1, 0, 0);
if numel(args) == 2
  printf('POWER_FLOW_RESULT=%s\n', jsonencode(solve_case(args{2})));
else
  cases = jsondecode(fileread(args{2}), 'makeValidName', false);
  keys = fieldnames(cases);
  results = struct();
  for k = 1:numel(keys)
    key = keys{k};
    try
      results.(key) = solve_case(cases.(key));
    catch err
      results.(key) = struct('status', 'solve_failed', 'detail', err.message);
    end_try_catch
  endfor
  fid = fopen(args{3}, 'w');
  fputs(fid, jsonencode(results));
  fclose(fid);
endif
