# The stock Contracts census under each engine: prefilled cold and warm states, CI and machine-capacity jobs.
require_relative 'session'

module HeadToHead
  module Census
    FAMILIES = %w[behavior check library interop testing project format graph].freeze
    VERDICT = %r{\A(PASS|FAIL|PEND|SKIP) ([a-z0-9_]+/[a-z0-9_]+/[a-z0-9_]+)(?:\s|\z)}
    TIMEOUT_SECONDS = 2400
    HOST_INTERVAL_SECONDS = 5

    # Every case verdict a census log reports; a repeated case refuses the log.
    def self.verdicts(path)
      cases = {}
      File.foreach(path) do |line|
        match = line.match(VERDICT)
        next unless match

        verdict, key = match.captures
        HeadToHead.refuse("#{path} repeats #{key}") if cases.key?(key)
        cases[key] = verdict
      end
      cases
    end

    def self.pass_floor(source)
      File.readlines(File.join(source, 'workspaces/contracts/pass-baseline.txt'), chomp: true).map(&:strip)
          .reject { |key| key.empty? || key.start_with?('#') }
    end

    # The stock runner invocation the installed verifier uses, with this arm's selector, Bakery and scratch.
    def self.invocation(session, engine, arm, jobs)
      environment = session.environment(engine, arm.fetch(:bakery)).merge(
        'INCAN_STDLIB' => '', 'INCAN_LSP' => session.lsp, 'OVEN_ENGINE_BINARY' => session.launcher,
        'TMPDIR' => arm.fetch(:tmp), 'RUSTC_BOOTSTRAP' => nil
      )
      argv = [session.settings.fetch(:contracts_runner), session.launcher, File.join(session.source, 'workspaces/contracts/fixtures'),
              '--scratch', arm.fetch(:scratch), '--jobs', jobs.to_s, '--timeout', TIMEOUT_SECONDS.to_s, '--serial-families',
              *FAMILIES]
      [environment, argv]
    end

    # Per engine, the warm census reuses the cold census's Bakery and its retained scratch: the same realized
    # projects, targets and paths, which the stock materializer keeps when their bytes are unchanged. Only the logs
    # and temporary directories are per state.
    def self.run_pair(session, pair, jobs, expected)
      root = session.directory('census', "jobs-#{jobs}", format('pair-%02d', pair))
      session.order(pair).each_with_index do |engine, position|
        base = File.join(root, engine)
        bakery = HeadToHead.clone_tree(session.prefill, File.join(base, 'bakery'))
        scratch = HeadToHead.make_directory(File.join(base, 'retained-scratch'))
        session.settings.fetch(:census_states).each do |state|
          arm = { bakery: bakery, scratch: scratch, tmp: HeadToHead.make_directory(File.join(base, "#{state}-tmp")),
                  log: File.join(base, "#{state}-contracts.log") }
          scope = { arm: 'census', pair: pair, position: position, engine: engine, jobs: jobs, stage: "census-#{state}",
                    state: state == 'cold' ? 'prefilled-cold' : 'warm-unchanged' }
          before = retained(session, arm)
          census_run(session, scope, engine, arm, jobs, expected)
          after = retained(session, arm)
          session.log(scope.merge(role: 'retained-state', before: before, after: after,
                                  workers_kept: before.fetch(:workers) & after.fetch(:workers)))
        end
      end
      session.retire(root)
    end

    # What a census state starts or ends with: the scratch's realized size and project targets, the Bakery's
    # generation and record counts, and the home's live workers (the admitted service generation).
    def self.retained(session, arm)
      kib, = Open3.capture2('/usr/bin/du', '-sk', arm.fetch(:scratch))
      targets, = Open3.capture2('/usr/bin/find', arm.fetch(:scratch), '-type', 'd', '-name', 'target', '-prune')
      { scratch_kib: kib.to_i, project_targets: targets.lines.size,
        bakery_current: File.read(File.join(arm.fetch(:bakery), 'CURRENT')).strip,
        store_counts: HeadToHead.store_counts(arm.fetch(:bakery)),
        workers: HeadToHead.home_workers([session.home]).map { |row| "#{row.fetch(:pid)} #{row.fetch(:command)[0, 160]}" } }
    end

    # One complete census. The log is retained; the verdict set must equal the oracle and keep every committed PASS.
    def self.census_run(session, scope, engine, arm, jobs, expected)
      stopped = HeadToHead.quiesce([session.home]) if scope.fetch(:state) == 'prefilled-cold'
      environment, argv = invocation(session, engine, arm, jobs)
      host = { before: HeadToHead.host_sample(session.slot_root), during: [] } if session.timed
      started = HeadToHead.clock_ns if session.timed
      pid = Process.spawn(environment, *argv, chdir: session.source, out: arm.fetch(:log), err: %i[child out])
      status, ended = wait(pid, host, session.slot_root)
      entry = scope.merge(role: 'ordinary', argv: argv, cwd: session.source, environment: environment, log: arm.fetch(:log),
                          exit: status.exitstatus, signal: status.termsig, workers_stopped_before: stopped, timed: session.timed)
      if session.timed
        entry[:wall_ms] = (ended - started) / 1_000_000.0
        entry[:host] = host.merge(after: HeadToHead.host_sample(session.slot_root))
      end
      judge(session, entry, verdicts(arm.fetch(:log)), expected)
    end

    # Block on the census itself so its end is read at once; host samples come from a side thread meanwhile.
    def self.wait(pid, host, slot_root)
      sampler = host && Thread.new do
        loop do
          host.fetch(:during) << HeadToHead.host_sample(slot_root)
          sleep HOST_INTERVAL_SECONDS
        end
      end
      _, status = Process.waitpid2(pid)
      ended = HeadToHead.clock_ns
      sampler&.kill
      sampler&.join
      [status, ended]
    end

    # Runner status 0 or 1 is an ordinary census end; the verdicts decide the arm.
    def self.judge(session, entry, cases, expected)
      floor = expected.fetch(:floor)
      counts = cases.values.group_by(&:itself).transform_values(&:size)
      missing = floor.reject { |key| cases[key] == 'PASS' }
      differing = (cases.keys | expected.fetch(:oracle).keys).reject { |key| cases[key] == expected.fetch(:oracle)[key] }
      entry.merge!(verdict_counts: counts, cases: cases.size, verdicts_sha256: Digest::SHA256.hexdigest(cases.sort.join("\n")),
                   oracle_equal: differing.empty?, differing_from_oracle: differing.to_h { |key| [key, [cases[key], expected.fetch(:oracle)[key]]] },
                   missing_passes: missing)
      session.log(entry)
      complete = [0, 1].include?(entry.fetch(:exit)) && entry.fetch(:signal).nil? && !cases.empty?
      HeadToHead.require_fact(complete, "census #{entry.fetch(:stage)} #{entry.fetch(:engine)} ended without a verdict set", entry)
      HeadToHead.require_fact(missing.empty? && differing.empty?,
                              "census #{entry.fetch(:stage)} #{entry.fetch(:engine)} differs from the frozen oracle", entry)
    end
  end
end
