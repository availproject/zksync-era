WITH
	CONFIG AS (
		SELECT
			-- miniblock number to start metrics collection from
			40000000 AS START_FROM_MINIBLOCK_NUMBER,
			-- compute metrics over how many of the most recent blocks
			50000 AS LIMIT_MINIBLOCKS
	)
SELECT
	-- initial writes
	STDDEV_SAMP(INITIAL_WRITES_PER_TX) AS INITIAL_WRITES_STDDEV,
	PERCENTILE_CONT(0.00) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_00,
	PERCENTILE_CONT(0.01) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_01,
	PERCENTILE_CONT(0.05) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_05,
	PERCENTILE_CONT(0.25) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_25,
	PERCENTILE_CONT(0.50) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_50,
	PERCENTILE_CONT(0.75) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_75,
	PERCENTILE_CONT(0.95) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_95,
	PERCENTILE_CONT(0.99) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_99,
	PERCENTILE_CONT(1.00) WITHIN GROUP (
		ORDER BY
			INITIAL_WRITES_PER_TX
	) AS INITIAL_WRITES_100,
	-- repeated writes
	STDDEV_SAMP(REPEATED_WRITES_PER_TX) AS REPEATED_WRITES_STDDEV,
	PERCENTILE_CONT(0.00) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_00,
	PERCENTILE_CONT(0.01) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_01,
	PERCENTILE_CONT(0.05) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_05,
	PERCENTILE_CONT(0.25) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_25,
	PERCENTILE_CONT(0.50) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_50,
	PERCENTILE_CONT(0.75) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_75,
	PERCENTILE_CONT(0.95) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_95,
	PERCENTILE_CONT(0.99) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_99,
	PERCENTILE_CONT(1.00) WITHIN GROUP (
		ORDER BY
			REPEATED_WRITES_PER_TX
	) AS REPEATED_WRITES_100
FROM
	(
		SELECT
			*,
			INITIAL_WRITES::REAL / L2_TX_COUNT::REAL AS INITIAL_WRITES_PER_TX,
			(TOTAL_WRITES - INITIAL_WRITES)::REAL / L2_TX_COUNT::REAL AS REPEATED_WRITES_PER_TX
		FROM
			(
				SELECT
					MB.NUMBER AS MINIBLOCK_NUMBER,
					COUNT(SL.HASHED_KEY) AS TOTAL_WRITES,
					COUNT(DISTINCT SL.HASHED_KEY) FILTER (
						WHERE
							IW.HASHED_KEY IS NOT NULL
					) AS INITIAL_WRITES,
					MB.L2_TX_COUNT AS L2_TX_COUNT
				FROM
					MINIBLOCKS MB
					JOIN L1_BATCHES L1B ON L1B.NUMBER = MB.L1_BATCH_NUMBER
					JOIN STORAGE_LOGS SL ON SL.MINIBLOCK_NUMBER = MB.NUMBER
					LEFT JOIN INITIAL_WRITES IW ON IW.HASHED_KEY = SL.HASHED_KEY
					AND IW.L1_BATCH_NUMBER = MB.L1_BATCH_NUMBER
					AND MB.NUMBER = (
						-- initial writes are only tracked by L1 batch number, so find the first miniblock in that batch that contains a write to that key
						SELECT
							MINIBLOCK_NUMBER
						FROM
							STORAGE_LOGS
						WHERE
							HASHED_KEY = SL.HASHED_KEY
						ORDER BY
							MINIBLOCK_NUMBER ASC
						LIMIT
							1
					)
				WHERE
					MB.L2_TX_COUNT <> 0 -- avoid div0
					AND MB.NUMBER >= (
						SELECT
							START_FROM_MINIBLOCK_NUMBER
						FROM
							CONFIG
					)
				GROUP BY
					MB.NUMBER
				ORDER BY
					MB.NUMBER DESC
				LIMIT
					(
						SELECT
							LIMIT_MINIBLOCKS
						FROM
							CONFIG
					)
			) S,
			GENERATE_SERIES(1, S.L2_TX_COUNT) -- scale by # of tx
	) T;
