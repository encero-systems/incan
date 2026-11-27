# Hello cold, unchanged, private-body edit and signature edit, with native controls and fresh recomputation.
require_relative 'session'

module HeadToHead
  module Hello
    INITIAL = 'edit 00 plain'.freeze
    PRIVATE = 'edit 01 plain'.freeze
    # Each stage, the state it measures and the output it must print.
    STAGES = [%w[cold prefilled-cold], %w[unchanged warm-unchanged], %w[private-body-edit warm-edit],
              %w[signature-edit warm-edit]].freeze
    EDITS = %w[private-body-edit signature-edit].freeze

    def self.edition(message)
      "def greeting() -> str:\n    return \"#{message}\"\n\ndef main() -> None:\n    println(greeting())\n"
    end

    # The source bytes and expected stdout of a stage. The private edit keeps the public signature and the literal's
    # width; the signature edit changes the return type from string to integer and the caller uses the integer.
    def self.stage_source(stage)
      case stage
      when 'cold', 'unchanged' then [edition(INITIAL), INITIAL]
      when 'private-body-edit' then [edition(PRIVATE), PRIVATE]
      when 'signature-edit'
        ["def greeting() -> int:\n    return 7\n\ndef main() -> None:\n    println(greeting() + 1)\n", '8']
      else HeadToHead.refuse("unknown Hello stage #{stage}")
      end
    end

    # A fresh project and a clone of the sealed prefilled Bakery for one engine.
    def self.prepare(session, root, engine, stage)
      project = HeadToHead.make_directory(File.join(root, 'project'))
      bakery = HeadToHead.clone_tree(session.prefill, File.join(root, 'bakery'))
      source = File.join(project, 'hello.incn')
      File.binwrite(source, stage_source(stage).first)
      { engine: engine, project: project, source: source, bakery: bakery, artifacts: {},
        binary: File.join(project, 'target/incan/debug/bin/hello'), environment: session.environment(engine, bakery) }
    end

    # One untimed throwaway run per engine, so the home's own catalog already holds each engine's one-time
    # installation record before any sample. `overrides` select a secondary home.
    def self.prime(session, home, label, overrides = {})
      primed = ENGINES.map do |engine|
        arm = prepare(session, session.directory('setup', "prime-#{label}-#{engine}"), engine, 'cold')
        entry = session.sample({ arm: 'setup', role: 'prime', home: home, engine: engine }, arm.fetch(:project),
                               overrides.merge(arm.fetch(:environment)), [File.join(home, 'bin/incan'), 'run', 'hello.incn'],
                               timed: false)
        check_output(entry, INITIAL, "priming #{label} with #{engine}")
        entry.fetch(:exit)
      end
      HeadToHead.quiesce([home])
      primed
    end

    def self.run_pair(session, pair)
      root = session.directory('hello', format('pair-%02d', pair))
      session.order(pair).each_with_index do |engine, position|
        arm = prepare(session, File.join(root, engine), engine, 'cold')
        STAGES.each { |stage, state| stage_sample(session, pair, position, stage, state, arm) }
        fresh_controls(session, pair, position, root, arm) if pair == 1
      end
      session.retire(root)
    end

    # One ordinary `incan run`, then the produced executable run directly as the native and OS-launch floor.
    def self.stage_sample(session, pair, position, stage, state, arm)
      bytes, expected = stage_source(stage)
      File.binwrite(arm.fetch(:source), bytes) if EDITS.include?(stage)
      if %w[prefilled-cold fresh-recomputation].include?(state)
        HeadToHead.require_fact(!File.exist?(File.join(arm.fetch(:project), 'target')), "#{stage} already has a target")
        stopped = HeadToHead.quiesce([session.home])
      end
      scope = { arm: 'hello', pair: pair, position: position, engine: arm.fetch(:engine), stage: stage, state: state,
                source_sha256: HeadToHead.sha256(arm.fetch(:source)), prior_source_sha256: arm[:prior_source_sha256],
                workers_stopped_before: stopped }
      ordinary = session.sample(scope.merge(role: 'ordinary'), arm.fetch(:project), arm.fetch(:environment),
                                [session.launcher, 'run', 'hello.incn'])
      check_output(ordinary, expected, "ordinary #{stage} #{arm.fetch(:engine)}")
      artifact = native_artifact(arm, stage, ordinary)
      direct = session.sample(scope.merge(role: 'native-control', artifact_sha256: artifact), arm.fetch(:project),
                              {}, [arm.fetch(:binary)])
      check_output(direct, expected, "native #{stage} #{arm.fetch(:engine)}")
      HeadToHead.require_fact(HeadToHead.sha256(arm.fetch(:binary)) == artifact, "executable changed during #{stage}")
      HeadToHead.require_fact(HeadToHead.sha256(arm.fetch(:source)) == scope.fetch(:source_sha256), "source changed during #{stage}")
      session.log(scope.merge(role: 'workers-after', workers: HeadToHead.home_workers([session.home])))
      arm[:prior_source_sha256] = scope.fetch(:source_sha256)
      arm.fetch(:artifacts)[stage] = artifact
    end

    def self.check_output(entry, expected, what)
      exact = entry.fetch(:exit) == 0 && entry.fetch(:signal).nil? && entry.fetch(:raw_stdout) == "#{expected}\n" &&
              entry.fetch(:stderr).fetch(:bytes).zero?
      HeadToHead.require_fact(exact, "#{what} did not print exactly #{expected.inspect} with empty stderr", entry)
    end

    # The native executable a stage produced; an unchanged rebuild must keep the cold stage's exact bytes.
    def self.native_artifact(arm, stage, ordinary)
      HeadToHead.require_fact(File.executable?(arm.fetch(:binary)), "#{stage} produced no native executable", ordinary)
      artifact = HeadToHead.sha256(arm.fetch(:binary))
      if stage == 'unchanged'
        HeadToHead.require_fact(artifact == arm.fetch(:artifacts).fetch('cold'), 'unchanged rebuild changed the executable')
      end
      artifact
    end

    # Each final edit again on its own fresh clone with no prior target: the cold recomputation baseline (section 13.2,
    # gate 3).
    def self.fresh_controls(session, pair, position, root, warm)
      EDITS.each do |stage|
        fresh = prepare(session, File.join(root, "#{warm.fetch(:engine)}-fresh-#{stage}"), warm.fetch(:engine), stage)
        HeadToHead.require_fact(HeadToHead.sha256(fresh.fetch(:source)) == warm_source_sha(stage),
                                "fresh #{stage} input differs from the warm edit")
        stage_sample(session, pair, position, stage, 'fresh-recomputation', fresh)
        session.log({ arm: 'hello', pair: pair, engine: warm.fetch(:engine), stage: stage, role: 'fresh-parity',
                      warm_artifact_sha256: warm.fetch(:artifacts).fetch(stage),
                      fresh_artifact_sha256: fresh.fetch(:artifacts).fetch(stage),
                      artifact_bytes_equal: warm.fetch(:artifacts).fetch(stage) == fresh.fetch(:artifacts).fetch(stage),
                      output_equal: true })
      end
    end

    def self.warm_source_sha(stage)
      Digest::SHA256.hexdigest(stage_source(stage).first)
    end
  end
end
