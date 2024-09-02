WITH
	CONFIG AS (
		SELECT
			-- compute metrics over how many of the most recent blocks
			1000 AS LIMIT_MINIBLOCKS
	)
SELECT
	SUM(INITIAL_WRITES) / SUM(L2_TX_COUNT) AS AVG_INITIAL_WRITES,
	(SUM(TOTAL_WRITES) - SUM(INITIAL_WRITES)) / SUM(L2_TX_COUNT) AS AVG_REPEATED_WRITES
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
			MB.L2_TX_COUNT <> 0
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
	) S;